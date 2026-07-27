use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use agentum_core::sdd::SpecId;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::artifacts::{
    MISSING_HASH, atomic_write, content_hash, read_bytes, remove_file_nofollow,
};
use super::sha256;

const SNAPSHOT_FORMAT: &str = "agentum-source-snapshot";
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_SNAPSHOT_ENTRIES: usize = 4096;
const MAX_SNAPSHOT_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SNAPSHOT_TOTAL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("repository path is not a real directory: {0}")]
    UnsafeRepository(String),
    #[error("source checkout is dirty; choose committed HEAD or a validated snapshot explicitly")]
    DirtySource,
    #[error("source checkout changed while its snapshot was being captured")]
    SnapshotChanged,
    #[error("unsupported source snapshot content: {0}")]
    UnsupportedSnapshot(String),
    #[error("git operation failed: {0}")]
    Git(String),
    #[error("authoritative worktree already exists: {0}")]
    Collision(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot manifest: {0}")]
    Json(#[from] serde_json::Error),
    #[error("path: {0}")]
    Path(#[from] agentum_store::paths::PathError),
}

#[derive(Debug, Clone)]
pub struct AuthoritativeWorkspace {
    pub path: PathBuf,
    pub base_commit: String,
    pub branch_name: String,
    pub fingerprint: String,
    pub snapshot_digest: Option<String>,
    snapshot: Option<SourceSnapshot>,
}

#[derive(Debug, Clone)]
pub struct AttemptWorkspace {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCheckoutMode {
    RequireClean,
    CommittedBase,
    Snapshot,
}

impl SourceCheckoutMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::RequireClean => "require_clean",
            Self::CommittedBase => "committed_base",
            Self::Snapshot => "snapshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotManifest {
    format: String,
    schema_version: u32,
    base_commit: String,
    entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotEntry {
    path: String,
    operation: SnapshotOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    size: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    executable: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotOperation {
    Write,
    Delete,
}

#[derive(Debug, Clone)]
struct SourceSnapshot {
    manifest: SnapshotManifest,
    digest: String,
    blobs: HashMap<String, Vec<u8>>,
}

/// Resolve and validate the immutable workspace inputs without mutating Git or
/// the filesystem. The returned paths can be durably reserved before publish.
pub async fn plan_authoritative(
    repo_id: &str,
    repository: &Path,
    run_id: &str,
    spec_id: &SpecId,
    title: &str,
    base_ref: &str,
    source_checkout: SourceCheckoutMode,
) -> Result<AuthoritativeWorkspace, WorkspaceError> {
    let metadata = std::fs::symlink_metadata(repository)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WorkspaceError::UnsafeRepository(
            repository.display().to_string(),
        ));
    }
    let canonical = repository.canonicalize()?;
    if base_ref.trim().is_empty()
        || base_ref.starts_with('-')
        || base_ref.len() > 255
        || base_ref.chars().any(char::is_control)
    {
        return Err(WorkspaceError::Git("base ref is invalid".into()));
    }
    git_success(&canonical, &["rev-parse", "--is-inside-work-tree"]).await?;
    let base_commit = git_output(&canonical, &["rev-parse", "--verify", base_ref]).await?;
    let base_commit = base_commit.trim().to_owned();
    let snapshot = match source_checkout {
        SourceCheckoutMode::RequireClean => {
            if !checkout_status(&canonical).await?.is_empty() {
                return Err(WorkspaceError::DirtySource);
            }
            None
        }
        SourceCheckoutMode::CommittedBase => None,
        SourceCheckoutMode::Snapshot => {
            let first = capture_source_snapshot(&canonical, &base_commit).await?;
            let second = capture_source_snapshot(&canonical, &base_commit).await?;
            if first.digest != second.digest || first.manifest != second.manifest {
                return Err(WorkspaceError::SnapshotChanged);
            }
            let current_base = git_output(&canonical, &["rev-parse", "--verify", base_ref]).await?;
            if current_base.trim() != base_commit {
                return Err(WorkspaceError::SnapshotChanged);
            }
            Some(second)
        }
    };
    let repo_key = &sha256(repo_id.as_bytes())[..16];
    let path = agentum_store::paths::sdd_worktree_dir()?
        .join(repo_key)
        .join(run_id)
        .join("authoritative");
    let branch_name = spec_id.branch_name(title);
    let snapshot_digest = snapshot.as_ref().map(|value| value.digest.clone());
    let fingerprint = sha256(format!(
        "{}\n{base_commit}\n{}\n{}\n",
        canonical.display(),
        source_checkout.as_str(),
        snapshot_digest.as_deref().unwrap_or("none")
    ));
    Ok(AuthoritativeWorkspace {
        path,
        base_commit,
        branch_name,
        fingerprint,
        snapshot_digest,
        snapshot,
    })
}

async fn checkout_status(repository: &Path) -> Result<Vec<u8>, WorkspaceError> {
    let output = git_bytes(
        repository,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    Ok(output)
}

async fn capture_source_snapshot(
    repository: &Path,
    base_commit: &str,
) -> Result<SourceSnapshot, WorkspaceError> {
    let modes = tracked_modes(repository).await?;
    if modes.values().any(|mode| mode == "160000") {
        return Err(WorkspaceError::UnsupportedSnapshot(
            "repositories containing submodules cannot be snapshotted".into(),
        ));
    }
    let status = checkout_status(repository).await?;
    let mut records = status
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty());
    let mut paths = HashSet::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            return Err(WorkspaceError::UnsupportedSnapshot(
                "Git returned a malformed checkout status".into(),
            ));
        }
        let state = &record[..2];
        if state.contains(&b'U')
            || matches!(state, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU")
        {
            return Err(WorkspaceError::UnsupportedSnapshot(
                "unmerged paths cannot be snapshotted".into(),
            ));
        }
        if state.contains(&b'R') || state.contains(&b'C') {
            // Porcelain v1 -z appends the source path as a second record. We
            // reject the whole operation rather than ambiguously replaying it.
            let _ = records.next();
            return Err(WorkspaceError::UnsupportedSnapshot(
                "renames and copies must be resolved before snapshotting".into(),
            ));
        }
        let relative = std::str::from_utf8(&record[3..]).map_err(|_| {
            WorkspaceError::UnsupportedSnapshot("non-UTF-8 paths are unsupported".into())
        })?;
        validate_snapshot_path(relative)?;
        if !paths.insert(relative.to_owned()) {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "duplicate changed path: {relative}"
            )));
        }
    }
    if paths.len() > MAX_SNAPSHOT_ENTRIES {
        return Err(WorkspaceError::UnsupportedSnapshot(format!(
            "snapshot exceeds {MAX_SNAPSHOT_ENTRIES} changed paths"
        )));
    }

    let mut entries = Vec::with_capacity(paths.len());
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
    let mut total_bytes = 0usize;
    for relative in paths {
        if matches!(
            modes.get(&relative).map(String::as_str),
            Some("120000" | "160000")
        ) {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "link or submodule entry is unsupported: {relative}"
            )));
        }
        let path = repository.join(&relative);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(SnapshotEntry {
                    path: relative,
                    operation: SnapshotOperation::Delete,
                    content_hash: None,
                    size: 0,
                    executable: false,
                });
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "only regular text files can be snapshotted: {relative}"
            )));
        }
        let (bytes, content_hash) = read_bytes(&path).map_err(|error| {
            WorkspaceError::UnsupportedSnapshot(format!("unsafe path {relative}: {error}"))
        })?;
        if bytes.len() > MAX_SNAPSHOT_FILE_BYTES {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "file exceeds {MAX_SNAPSHOT_FILE_BYTES} bytes: {relative}"
            )));
        }
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_SNAPSHOT_TOTAL_BYTES {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "snapshot exceeds {MAX_SNAPSHOT_TOTAL_BYTES} bytes"
            )));
        }
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "binary content is unsupported: {relative}"
            )));
        }
        if let Some(existing) = blobs.insert(content_hash.clone(), bytes.clone())
            && existing != bytes
        {
            return Err(WorkspaceError::UnsupportedSnapshot(
                "content hash collision while capturing snapshot".into(),
            ));
        }
        entries.push(SnapshotEntry {
            path: relative,
            operation: SnapshotOperation::Write,
            content_hash: Some(content_hash),
            size: bytes.len(),
            executable: is_executable(&metadata),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = SnapshotManifest {
        format: SNAPSHOT_FORMAT.into(),
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        base_commit: base_commit.into(),
        entries,
    };
    let digest = sha256(serde_json::to_vec(&manifest)?);
    Ok(SourceSnapshot {
        manifest,
        digest,
        blobs,
    })
}

async fn tracked_modes(repository: &Path) -> Result<HashMap<String, String>, WorkspaceError> {
    let raw = git_bytes(repository, &["ls-files", "--stage", "-z"]).await?;
    let mut modes = HashMap::new();
    for record in raw.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
        let text = std::str::from_utf8(record).map_err(|_| {
            WorkspaceError::UnsupportedSnapshot("non-UTF-8 tracked path is unsupported".into())
        })?;
        let (metadata, path) = text.split_once('\t').ok_or_else(|| {
            WorkspaceError::UnsupportedSnapshot("malformed tracked-file metadata".into())
        })?;
        let mut fields = metadata.split_ascii_whitespace();
        let mode = fields.next().unwrap_or_default();
        let _object = fields.next().unwrap_or_default();
        let stage = fields.next().unwrap_or_default();
        if !matches!(mode, "100644" | "100755" | "120000" | "160000") || stage != "0" {
            return Err(WorkspaceError::UnsupportedSnapshot(format!(
                "unsupported tracked entry: {path}"
            )));
        }
        validate_snapshot_path(path)?;
        modes.insert(path.to_owned(), mode.to_owned());
    }
    Ok(modes)
}

fn validate_snapshot_path(relative: &str) -> Result<(), WorkspaceError> {
    agentum_core::sdd::validate_relative_path(relative).map_err(|_| {
        WorkspaceError::UnsupportedSnapshot(format!("unsafe relative path: {relative}"))
    })?;
    if relative.len() > 1024
        || relative.contains(['\\', ':'])
        || relative.chars().any(char::is_control)
        || Path::new(relative).components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(".git"))
        })
    {
        return Err(WorkspaceError::UnsupportedSnapshot(format!(
            "cross-platform unsafe path: {relative}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Materialize a previously reserved workspace plan. Every owned parent is
/// created one component at a time and existing links are rejected.
pub async fn materialize_authoritative(
    repository: &Path,
    workspace: &AuthoritativeWorkspace,
) -> Result<(), WorkspaceError> {
    if std::fs::symlink_metadata(&workspace.path).is_ok() {
        return Err(WorkspaceError::Collision(
            workspace.path.display().to_string(),
        ));
    }
    ensure_owned_worktree_parent(&workspace.path)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["worktree", "add", "-b"])
        .arg(&workspace.branch_name)
        .arg(&workspace.path)
        .arg(&workspace.base_commit)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(redacted_failure(&output)));
    }
    if let Some(snapshot) = &workspace.snapshot {
        persist_source_snapshot(workspace, snapshot)?;
        apply_source_snapshot(snapshot, &workspace.path)?;
    }
    Ok(())
}

fn snapshot_root(authoritative: &Path) -> Result<PathBuf, WorkspaceError> {
    validate_owned_authoritative_path(authoritative)?;
    Ok(authoritative
        .parent()
        .ok_or_else(|| WorkspaceError::UnsafeRepository(authoritative.display().to_string()))?
        .join("source-snapshot"))
}

fn persist_source_snapshot(
    workspace: &AuthoritativeWorkspace,
    snapshot: &SourceSnapshot,
) -> Result<(), WorkspaceError> {
    validate_source_snapshot(snapshot, &workspace.base_commit, Some(&snapshot.digest))?;
    let root = snapshot_root(&workspace.path)?;
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err(WorkspaceError::Collision(root.display().to_string())),
        Err(error) => return Err(error.into()),
    }
    ensure_directory_chain_nofollow(&root.join("files"))?;
    let mut hashes: Vec<_> = snapshot.blobs.keys().collect();
    hashes.sort();
    for hash in hashes {
        let bytes = &snapshot.blobs[hash];
        let published = atomic_write(&root.join("files").join(hash), bytes, Some(MISSING_HASH))
            .map_err(|error| {
                WorkspaceError::UnsupportedSnapshot(format!(
                    "snapshot blob could not be published: {error}"
                ))
            })?;
        if &published != hash {
            return Err(WorkspaceError::UnsupportedSnapshot(
                "snapshot blob digest changed during publication".into(),
            ));
        }
    }
    let manifest = serde_json::to_vec(&snapshot.manifest)?;
    let published = atomic_write(&root.join("manifest.json"), &manifest, Some(MISSING_HASH))
        .map_err(|error| {
            WorkspaceError::UnsupportedSnapshot(format!(
                "snapshot manifest could not be published: {error}"
            ))
        })?;
    if published != snapshot.digest {
        return Err(WorkspaceError::UnsupportedSnapshot(
            "snapshot manifest digest changed during publication".into(),
        ));
    }
    Ok(())
}

fn load_source_snapshot(
    authoritative: &Path,
    base_commit: &str,
    expected_digest: &str,
) -> Result<SourceSnapshot, WorkspaceError> {
    let root = snapshot_root(authoritative)?;
    let (manifest_bytes, digest) = read_bytes(&root.join("manifest.json")).map_err(|error| {
        WorkspaceError::UnsupportedSnapshot(format!("snapshot manifest is unsafe: {error}"))
    })?;
    if digest != expected_digest {
        return Err(WorkspaceError::UnsupportedSnapshot(
            "snapshot manifest does not match its durable digest".into(),
        ));
    }
    let manifest: SnapshotManifest = serde_json::from_slice(&manifest_bytes)?;
    let mut blobs = HashMap::new();
    for entry in &manifest.entries {
        if let Some(hash) = &entry.content_hash {
            if blobs.contains_key(hash) {
                continue;
            }
            let (bytes, current_hash) =
                read_bytes(&root.join("files").join(hash)).map_err(|error| {
                    WorkspaceError::UnsupportedSnapshot(format!("snapshot blob is unsafe: {error}"))
                })?;
            if &current_hash != hash {
                return Err(WorkspaceError::UnsupportedSnapshot(
                    "snapshot blob digest is invalid".into(),
                ));
            }
            blobs.insert(hash.clone(), bytes);
        }
    }
    let snapshot = SourceSnapshot {
        manifest,
        digest,
        blobs,
    };
    validate_source_snapshot(&snapshot, base_commit, Some(expected_digest))?;
    Ok(snapshot)
}

fn validate_source_snapshot(
    snapshot: &SourceSnapshot,
    base_commit: &str,
    expected_digest: Option<&str>,
) -> Result<(), WorkspaceError> {
    if snapshot.manifest.format != SNAPSHOT_FORMAT
        || snapshot.manifest.schema_version != SNAPSHOT_SCHEMA_VERSION
        || snapshot.manifest.base_commit != base_commit
        || snapshot.manifest.entries.len() > MAX_SNAPSHOT_ENTRIES
        || expected_digest.is_some_and(|expected| expected != snapshot.digest)
        || sha256(serde_json::to_vec(&snapshot.manifest)?) != snapshot.digest
    {
        return Err(WorkspaceError::UnsupportedSnapshot(
            "snapshot identity or schema is invalid".into(),
        ));
    }
    let mut paths = HashSet::new();
    let mut referenced = HashSet::new();
    let mut total = 0usize;
    for entry in &snapshot.manifest.entries {
        validate_snapshot_path(&entry.path)?;
        if !paths.insert(&entry.path) {
            return Err(WorkspaceError::UnsupportedSnapshot(
                "snapshot contains duplicate paths".into(),
            ));
        }
        match entry.operation {
            SnapshotOperation::Delete
                if entry.content_hash.is_some() || entry.size != 0 || entry.executable =>
            {
                return Err(WorkspaceError::UnsupportedSnapshot(
                    "delete entry carries file content".into(),
                ));
            }
            SnapshotOperation::Delete => {}
            SnapshotOperation::Write => {
                let hash = entry.content_hash.as_ref().ok_or_else(|| {
                    WorkspaceError::UnsupportedSnapshot("write entry has no content hash".into())
                })?;
                let bytes = snapshot.blobs.get(hash).ok_or_else(|| {
                    WorkspaceError::UnsupportedSnapshot("snapshot blob is missing".into())
                })?;
                if hash.len() != 64
                    || sha256(bytes) != *hash
                    || bytes.len() != entry.size
                    || bytes.len() > MAX_SNAPSHOT_FILE_BYTES
                    || bytes.contains(&0)
                    || std::str::from_utf8(bytes).is_err()
                {
                    return Err(WorkspaceError::UnsupportedSnapshot(
                        "snapshot blob is malformed or binary".into(),
                    ));
                }
                total = total.saturating_add(bytes.len());
                referenced.insert(hash);
            }
        }
    }
    if total > MAX_SNAPSHOT_TOTAL_BYTES || referenced.len() != snapshot.blobs.len() {
        return Err(WorkspaceError::UnsupportedSnapshot(
            "snapshot size or blob set is invalid".into(),
        ));
    }
    Ok(())
}

fn apply_source_snapshot(
    snapshot: &SourceSnapshot,
    destination: &Path,
) -> Result<(), WorkspaceError> {
    validate_source_snapshot(
        snapshot,
        &snapshot.manifest.base_commit,
        Some(&snapshot.digest),
    )?;
    ensure_real_directory(destination)?;
    for entry in &snapshot.manifest.entries {
        let target = destination.join(&entry.path);
        match entry.operation {
            SnapshotOperation::Delete => {
                let current = content_hash(&target).map_err(|error| {
                    WorkspaceError::UnsupportedSnapshot(format!(
                        "unsafe delete target {}: {error}",
                        entry.path
                    ))
                })?;
                if current != MISSING_HASH {
                    remove_file_nofollow(&target).map_err(|error| {
                        WorkspaceError::UnsupportedSnapshot(format!(
                            "could not remove snapshot path {}: {error}",
                            entry.path
                        ))
                    })?;
                }
            }
            SnapshotOperation::Write => {
                let parent = target.parent().ok_or_else(|| {
                    WorkspaceError::UnsupportedSnapshot(format!(
                        "snapshot path has no parent: {}",
                        entry.path
                    ))
                })?;
                ensure_directory_chain_nofollow(parent)?;
                let current = content_hash(&target).map_err(|error| {
                    WorkspaceError::UnsupportedSnapshot(format!(
                        "unsafe snapshot target {}: {error}",
                        entry.path
                    ))
                })?;
                let hash = entry.content_hash.as_ref().expect("validated write hash");
                atomic_write(
                    &target,
                    snapshot.blobs.get(hash).expect("validated snapshot blob"),
                    Some(&current),
                )
                .map_err(|error| {
                    WorkspaceError::UnsupportedSnapshot(format!(
                        "could not apply snapshot path {}: {error}",
                        entry.path
                    ))
                })?;
                set_executable(&target, entry.executable)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<(), WorkspaceError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<(), WorkspaceError> {
    Ok(())
}

pub async fn create_authoritative(
    repo_id: &str,
    repository: &Path,
    run_id: &str,
    spec_id: &SpecId,
    title: &str,
    base_ref: &str,
    source_checkout: SourceCheckoutMode,
) -> Result<AuthoritativeWorkspace, WorkspaceError> {
    let workspace = plan_authoritative(
        repo_id,
        repository,
        run_id,
        spec_id,
        title,
        base_ref,
        source_checkout,
    )
    .await?;
    materialize_authoritative(repository, &workspace).await?;
    Ok(workspace)
}

pub fn attempt_path(authoritative: &Path, attempt_id: &str) -> Result<PathBuf, WorkspaceError> {
    if attempt_id.is_empty()
        || attempt_id.len() > 128
        || attempt_id.starts_with('-')
        || !attempt_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '-')
    {
        return Err(WorkspaceError::UnsafeRepository(
            "attempt identity is invalid".into(),
        ));
    }
    let run_root = authoritative
        .parent()
        .ok_or_else(|| WorkspaceError::UnsafeRepository(authoritative.display().to_string()))?;
    Ok(run_root.join("attempts").join(attempt_id))
}

/// Compensation for a create transaction that failed after `git worktree add`.
/// The explicit path was allocated by this run and is never inferred/globbed.
pub async fn compensate_create(
    repository: &Path,
    workspace: &Path,
    branch: &str,
) -> Result<(), WorkspaceError> {
    recover_interrupted_create(repository, workspace, branch).await
}

/// Reconcile one exact create reservation. Missing worktrees/branches are
/// already-clean success; any other Git failure leaves the saga quarantined for
/// operator recovery instead of deleting guessed paths.
pub async fn recover_interrupted_create(
    repository: &Path,
    workspace: &Path,
    branch: &str,
) -> Result<(), WorkspaceError> {
    validate_owned_authoritative_path(workspace)?;
    validate_branch_name(branch)?;
    let original_metadata = std::fs::symlink_metadata(repository)?;
    if original_metadata.file_type().is_symlink() || !original_metadata.is_dir() {
        return Err(WorkspaceError::UnsafeRepository(
            repository.display().to_string(),
        ));
    }
    let repository = repository.canonicalize()?;
    let metadata = std::fs::symlink_metadata(&repository)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WorkspaceError::UnsafeRepository(
            repository.display().to_string(),
        ));
    }
    if std::fs::symlink_metadata(workspace).is_ok() {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["worktree", "remove", "--force"])
            .arg(workspace)
            .stdin(Stdio::null())
            .output()
            .await;
        match output {
            Ok(output) if output.status.success() => {}
            Ok(output) => return Err(WorkspaceError::Git(redacted_failure(&output))),
            Err(error) => return Err(error.into()),
        }
    }
    let branch_ref = format!("refs/heads/{branch}");
    let exists = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(&branch_ref)
        .stdin(Stdio::null())
        .status()
        .await?;
    if exists.success() {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["branch", "-D", branch])
            .stdin(Stdio::null())
            .output()
            .await?;
        if !output.status.success() {
            return Err(WorkspaceError::Git(redacted_failure(&output)));
        }
    }
    Ok(())
}

pub async fn recover_interrupted_attempt(
    repository: &Path,
    authoritative: &Path,
    attempt: &Path,
) -> Result<(), WorkspaceError> {
    validate_owned_authoritative_path(authoritative)?;
    let attempts = authoritative
        .parent()
        .expect("validated authoritative path")
        .join("attempts");
    let relative = attempt
        .strip_prefix(&attempts)
        .map_err(|_| WorkspaceError::UnsafeRepository(attempt.display().to_string()))?;
    if relative.components().count() != 1
        || !matches!(
            relative.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(WorkspaceError::UnsafeRepository(
            attempt.display().to_string(),
        ));
    }
    if std::fs::symlink_metadata(attempt).is_err() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(repository)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WorkspaceError::UnsafeRepository(
            repository.display().to_string(),
        ));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["worktree", "remove", "--force"])
        .arg(attempt)
        .stdin(Stdio::null())
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(WorkspaceError::Git(redacted_failure(&output)))
    }
}

fn validate_owned_authoritative_path(path: &Path) -> Result<(), WorkspaceError> {
    let root = agentum_store::paths::sdd_worktree_dir()?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| WorkspaceError::UnsafeRepository(path.display().to_string()))?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 3
        || !matches!(parts[0], std::path::Component::Normal(_))
        || !matches!(parts[1], std::path::Component::Normal(_))
        || parts[2].as_os_str() != "authoritative"
    {
        return Err(WorkspaceError::UnsafeRepository(path.display().to_string()));
    }
    let repo_directory = root.join(parts[0].as_os_str());
    for candidate in [
        root.as_path(),
        repo_directory.as_path(),
        path.parent().unwrap(),
        path,
    ] {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(WorkspaceError::UnsafeRepository(
                    candidate.display().to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_branch_name(branch: &str) -> Result<(), WorkspaceError> {
    if !branch.starts_with("agentum/spc-")
        || branch.len() > 160
        || branch.contains("..")
        || branch.contains("@{")
        || branch.contains("//")
        || branch.ends_with(".lock")
        || branch.ends_with(['.', '/'])
        || branch
            .chars()
            .any(|value| value.is_control() || value.is_whitespace() || "~^:?*[\\".contains(value))
    {
        return Err(WorkspaceError::Git(
            "reserved branch name is invalid".into(),
        ));
    }
    Ok(())
}

pub async fn create_attempt(
    repository: &Path,
    authoritative: &Path,
    attempt_id: &str,
    base_commit: &str,
    snapshot_digest: Option<&str>,
) -> Result<AttemptWorkspace, WorkspaceError> {
    let path = attempt_path(authoritative, attempt_id)?;
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(WorkspaceError::Collision(path.display().to_string()));
    }
    ensure_real_directory(authoritative)?;
    let attempts = path.parent().expect("attempt has parent");
    validate_owned_authoritative_path(authoritative)?;
    ensure_directory_chain_nofollow(attempts)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["worktree", "add", "--detach"])
        .arg(&path)
        .arg(base_commit)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(redacted_failure(&output)));
    }
    if let Some(expected_digest) = snapshot_digest {
        let snapshot = load_source_snapshot(authoritative, base_commit, expected_digest)?;
        apply_source_snapshot(&snapshot, &path)?;
    }
    Ok(AttemptWorkspace { path })
}

fn ensure_real_directory(path: &Path) -> Result<(), WorkspaceError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WorkspaceError::UnsafeRepository(path.display().to_string()));
    }
    Ok(())
}

fn ensure_owned_worktree_parent(path: &Path) -> Result<(), WorkspaceError> {
    let root = agentum_store::paths::sdd_worktree_dir()?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| WorkspaceError::UnsafeRepository(path.display().to_string()))?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        || parts[2].as_os_str() != "authoritative"
    {
        return Err(WorkspaceError::UnsafeRepository(path.display().to_string()));
    }
    ensure_directory_chain_nofollow(
        path.parent()
            .ok_or_else(|| WorkspaceError::UnsafeRepository(path.display().to_string()))?,
    )
}

pub async fn remove_attempt(repository: &Path, workspace: &Path) -> Result<(), WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["worktree", "remove", "--force"])
        .arg(workspace)
        .stdin(Stdio::null())
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(WorkspaceError::Git(redacted_failure(&output)))
    }
}

#[cfg(unix)]
pub(crate) fn ensure_directory_chain_nofollow(path: &Path) -> Result<(), WorkspaceError> {
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let start = CString::new(if path.is_absolute() { "/" } else { "." })
        .expect("static directory contains no NUL");
    // SAFETY: `start` is a live C string and ownership of the returned fd is
    // transferred to `File` after checking it.
    let raw = unsafe {
        libc::open(
            start.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: open returned a fresh owned descriptor.
    let mut directory = unsafe { File::from_raw_fd(raw) };
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            if matches!(component, std::path::Component::RootDir) {
                continue;
            }
            return Err(WorkspaceError::UnsafeRepository(path.display().to_string()));
        };
        let name = CString::new(part.as_bytes())
            .map_err(|_| WorkspaceError::UnsafeRepository(path.display().to_string()))?;
        // SAFETY: the descriptor and C string remain live through the call.
        let mut child = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if child < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(WorkspaceError::UnsafeRepository(format!(
                    "{} ({error})",
                    path.display()
                )));
            }
            // SAFETY: mkdirat is relative to the held, no-follow parent.
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let create_error = std::io::Error::last_os_error();
                if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(create_error.into());
                }
            }
            directory.sync_all()?;
            // SAFETY: reopen the just-created entry without following links;
            // an attacker winning the name race is rejected here.
            child = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if child < 0 {
                return Err(WorkspaceError::UnsafeRepository(format!(
                    "{} ({})",
                    path.display(),
                    std::io::Error::last_os_error()
                )));
            }
        }
        // SAFETY: openat returned a fresh owned descriptor.
        directory = unsafe { File::from_raw_fd(child) };
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn ensure_directory_chain_nofollow(path: &Path) -> Result<(), WorkspaceError> {
    use cap_primitives::ambient_authority;

    let mut anchor = PathBuf::new();
    let mut children = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) if children.is_empty() => {
                anchor.push(prefix.as_os_str());
            }
            std::path::Component::RootDir if children.is_empty() => {
                anchor.push(Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            std::path::Component::Normal(part) => children.push(part.to_owned()),
            _ => return Err(WorkspaceError::UnsafeRepository(path.display().to_string())),
        }
    }
    if !path.is_absolute() || anchor.as_os_str().is_empty() {
        return Err(WorkspaceError::UnsafeRepository(path.display().to_string()));
    }
    let mut directory = cap_primitives::fs::open_ambient_dir(&anchor, ambient_authority())?;
    for child in children {
        match cap_primitives::fs::open_dir_nofollow(&directory, Path::new(&child)) {
            Ok(opened) => directory = opened,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                cap_primitives::fs::create_dir(
                    &directory,
                    Path::new(&child),
                    &cap_primitives::fs::DirOptions::new(),
                )?;
                directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(&child))?;
            }
            Err(error) => {
                return Err(WorkspaceError::UnsafeRepository(format!(
                    "{} ({error})",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn ensure_directory_chain_nofollow(path: &Path) -> Result<(), WorkspaceError> {
    Err(WorkspaceError::UnsafeRepository(format!(
        "safe workspace creation is unsupported on this operating system: {}",
        path.display()
    )))
}

async fn git_success(repository: &Path, args: &[&str]) -> Result<(), WorkspaceError> {
    let output = git(repository, args).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(WorkspaceError::Git(redacted_failure(&output)))
    }
}

async fn git_output(repository: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    String::from_utf8(git_bytes(repository, args).await?)
        .map_err(|_| WorkspaceError::Git("git returned non-UTF-8 output".into()))
}

async fn git_bytes(repository: &Path, args: &[&str]) -> Result<Vec<u8>, WorkspaceError> {
    let output = git(repository, args).await?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(redacted_failure(&output)));
    }
    Ok(output.stdout)
}

async fn git(repository: &Path, args: &[&str]) -> Result<std::process::Output, WorkspaceError> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?)
}

fn redacted_failure(output: &std::process::Output) -> String {
    // Git diagnostics may echo credential-bearing remotes. Detailed stderr is
    // intentionally not returned through the API; callers still receive a
    // stable status without risking secret disclosure.
    format!("git exited with {}", output.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git_at(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serializes process-wide AGENTUM_HOME overrides
    async fn dirty_source_is_rejected_before_external_files_are_created() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        git_at(repo.path(), &["init", "-q"]).await;
        git_at(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_at(repo.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(repo.path().join("README.md"), "base").unwrap();
        git_at(repo.path(), &["add", "README.md"]).await;
        git_at(repo.path(), &["commit", "-qm", "base"]).await;
        std::fs::write(repo.path().join("dirty.txt"), "dirty").unwrap();
        unsafe {
            std::env::set_var("AGENTUM_HOME", home.path());
        }
        let result = create_authoritative(
            "repo",
            repo.path(),
            "run",
            &SpecId::new(),
            "T",
            "HEAD",
            SourceCheckoutMode::RequireClean,
        )
        .await;
        unsafe {
            std::env::remove_var("AGENTUM_HOME");
        }
        assert!(matches!(result, Err(WorkspaceError::DirtySource)));
        assert!(!home.path().join("data/worktrees").exists());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn explicit_committed_base_ignores_dirty_checkout_bytes() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        git_at(repo.path(), &["init", "-q"]).await;
        git_at(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_at(repo.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(repo.path().join("README.md"), "committed").unwrap();
        git_at(repo.path(), &["add", "README.md"]).await;
        git_at(repo.path(), &["commit", "-qm", "base"]).await;
        std::fs::write(repo.path().join("README.md"), "dirty").unwrap();
        std::fs::write(repo.path().join("untracked.txt"), "not selected").unwrap();
        unsafe {
            std::env::set_var("AGENTUM_HOME", home.path());
        }
        let workspace = create_authoritative(
            "repo",
            repo.path(),
            "run",
            &SpecId::new(),
            "T",
            "HEAD",
            SourceCheckoutMode::CommittedBase,
        )
        .await
        .unwrap();
        unsafe {
            std::env::remove_var("AGENTUM_HOME");
        }
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("README.md")).unwrap(),
            "committed"
        );
        assert!(!workspace.path.join("untracked.txt").exists());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn explicit_snapshot_is_hashed_recoverable_and_replayed_into_attempts() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        git_at(repo.path(), &["init", "-q"]).await;
        git_at(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_at(repo.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(repo.path().join("README.md"), "base\n").unwrap();
        std::fs::write(repo.path().join("removed.txt"), "remove me\n").unwrap();
        git_at(repo.path(), &["add", "README.md", "removed.txt"]).await;
        git_at(repo.path(), &["commit", "-qm", "base"]).await;
        std::fs::write(repo.path().join("README.md"), "dirty text\n").unwrap();
        std::fs::remove_file(repo.path().join("removed.txt")).unwrap();
        std::fs::write(repo.path().join("untracked.txt"), "captured\n").unwrap();
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };
        let workspace = create_authoritative(
            "snapshot-repo",
            repo.path(),
            "snapshot-run",
            &SpecId::new(),
            "Snapshot",
            "HEAD",
            SourceCheckoutMode::Snapshot,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("README.md")).unwrap(),
            "dirty text\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("untracked.txt")).unwrap(),
            "captured\n"
        );
        assert!(!workspace.path.join("removed.txt").exists());
        let digest = workspace.snapshot_digest.clone().unwrap();
        let snapshot_directory = workspace.path.parent().unwrap().join("source-snapshot");
        assert_eq!(
            content_hash(&snapshot_directory.join("manifest.json")).unwrap(),
            digest
        );
        let attempt = create_attempt(
            repo.path(),
            &workspace.path,
            "attempt-1",
            &workspace.base_commit,
            Some(&digest),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(attempt.path.join("README.md")).unwrap(),
            "dirty text\n"
        );
        assert!(!attempt.path.join("removed.txt").exists());
        assert_eq!(
            std::fs::read_to_string(attempt.path.join("untracked.txt")).unwrap(),
            "captured\n"
        );
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
    }

    #[tokio::test]
    async fn snapshot_rejects_binary_and_submodule_entries() {
        let repo = tempfile::tempdir().unwrap();
        git_at(repo.path(), &["init", "-q"]).await;
        git_at(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_at(repo.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(repo.path().join("README.md"), "base\n").unwrap();
        git_at(repo.path(), &["add", "README.md"]).await;
        git_at(repo.path(), &["commit", "-qm", "base"]).await;
        std::fs::write(repo.path().join("binary.dat"), [0, 1, 2, 3]).unwrap();
        let binary = plan_authoritative(
            "repo",
            repo.path(),
            "run-binary",
            &SpecId::new(),
            "Binary",
            "HEAD",
            SourceCheckoutMode::Snapshot,
        )
        .await;
        assert!(
            matches!(binary, Err(WorkspaceError::UnsupportedSnapshot(message)) if message.contains("binary"))
        );
        std::fs::remove_file(repo.path().join("binary.dat")).unwrap();
        let commit = git_output(repo.path(), &["rev-parse", "HEAD"])
            .await
            .unwrap();
        git_at(
            repo.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{},vendor/submodule", commit.trim()),
            ],
        )
        .await;
        let submodule = plan_authoritative(
            "repo",
            repo.path(),
            "run-submodule",
            &SpecId::new(),
            "Submodule",
            "HEAD",
            SourceCheckoutMode::Snapshot,
        )
        .await;
        assert!(
            matches!(submodule, Err(WorkspaceError::UnsupportedSnapshot(message)) if message.contains("submodule"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn snapshot_rejects_symlink_overlay_without_following_it() {
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        git_at(repo.path(), &["init", "-q"]).await;
        git_at(
            repo.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .await;
        git_at(repo.path(), &["config", "user.name", "Test"]).await;
        std::fs::write(repo.path().join("README.md"), "base\n").unwrap();
        git_at(repo.path(), &["add", "README.md"]).await;
        git_at(repo.path(), &["commit", "-qm", "base"]).await;
        symlink(outside.path(), repo.path().join("unsafe-link")).unwrap();
        let result = plan_authoritative(
            "repo",
            repo.path(),
            "run-link",
            &SpecId::new(),
            "Link",
            "HEAD",
            SourceCheckoutMode::Snapshot,
        )
        .await;
        assert!(
            matches!(result, Err(WorkspaceError::UnsupportedSnapshot(message)) if message.contains("regular text"))
        );
    }

    #[tokio::test]
    async fn option_like_base_ref_is_rejected_before_git_uses_it() {
        let repo = tempfile::tempdir().unwrap();
        let result = create_authoritative(
            "repo",
            repo.path(),
            "run",
            &SpecId::new(),
            "T",
            "--help",
            SourceCheckoutMode::RequireClean,
        )
        .await;
        assert!(
            matches!(result, Err(WorkspaceError::Git(message)) if message == "base ref is invalid")
        );
    }

    #[test]
    fn recovery_targets_are_bound_to_the_owned_layout() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };
        let root = agentum_store::paths::sdd_worktree_dir().unwrap();
        let owned = root.join("0123456789abcdef/run-1/authoritative");
        assert!(validate_owned_authoritative_path(&owned).is_ok());
        assert!(validate_owned_authoritative_path(Path::new("/tmp/not-agentum")).is_err());
        assert!(validate_branch_name("agentum/spc-01arz3ndektsv4rrffq69g5fav-example").is_ok());
        assert!(validate_branch_name("refs/heads/main").is_err());
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
    }

    #[cfg(unix)]
    #[test]
    fn worktree_parent_creation_rejects_symlinked_and_unowned_paths() {
        use std::os::unix::fs::symlink;

        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };
        symlink(outside.path(), home.path().join("data")).unwrap();
        let root = agentum_store::paths::sdd_worktree_dir().unwrap();
        let target = root.join("0123456789abcdef/run-1/authoritative");
        assert!(matches!(
            ensure_owned_worktree_parent(&target),
            Err(WorkspaceError::UnsafeRepository(_))
        ));
        assert!(outside.path().read_dir().unwrap().next().is_none());
        assert!(ensure_owned_worktree_parent(&root.join("repo/too/deep/authoritative")).is_err());
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
    }

    #[tokio::test]
    async fn attempt_cleanup_failure_is_reported_for_quarantine() {
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing-repository");
        let result = remove_attempt(&missing, Path::new("/tmp/missing-attempt")).await;
        assert!(result.is_err());
    }
}
