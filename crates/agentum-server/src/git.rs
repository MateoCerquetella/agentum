//! Thin wrapper over the `git` CLI for per-session worktree isolation.
//!
//! Lets the user opt a session into its own branch + checkout so five
//! agents can run on the same repo in parallel without stomping each
//! other's working tree. We shell out to `git` rather than pulling in
//! `git2`/libgit2 to keep the binary tiny and avoid a system dep that
//! cross-compilation has historically tripped on (see CLAUDE.md note
//! on the `cc` wrapper trap).
//!
//! Public surface:
//!   * [`create_worktree`] — `git worktree add -b <branch> <path> <base_ref>`
//!   * [`prune_worktree`]  — `git worktree remove <path>` (with `--force`
//!     fallback when the user wants to abandon uncommitted changes)
//!   * [`worktree_status`] — `git status --porcelain` parsed to a tiny
//!     count struct, used by the future "uncommitted changes?" preflight
//!
//! Path layout: worktrees land in `<repo-parent>/<repo-name>-worktrees/<branch-slug>`
//! so a `myproj/` repo with worktree branch `agentum/feat-foo` ends up at
//! `myproj-worktrees/agentum-feat-foo`. Keeping them as a sibling of the
//! repo (not inside `.git/worktrees`, which git uses for bookkeeping)
//! means the user can `cd` into them with their normal shell tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("worktree path already exists: {0}")]
    PathExists(PathBuf),
    #[error("branch already exists: {0}")]
    BranchExists(String),
    #[error("git command failed: {0}")]
    CommandFailed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What `create_worktree` returns once `git worktree add` succeeded.
/// The HTTP layer copies these straight into `NewSession.worktree_*`
/// before calling the store.
#[derive(Debug, Clone)]
pub struct ResolvedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub base_ref: String,
}

/// True when `path` (or any ancestor) is inside a git working tree.
/// Cheap — runs `git -C <path> rev-parse --is-inside-work-tree`. Used
/// by the route handler to fail fast with a clear error before
/// attempting `git worktree add` on a non-repo workdir.
pub async fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Slugify a session name into something git accepts as a branch and
/// the filesystem accepts as a directory: lowercase, alnum + dash,
/// collapse repeats, trim leading/trailing dashes, cap at 60 chars.
/// Empty input becomes `"session"`.
pub fn slugify_branch(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        return "session".to_string();
    }
    out.truncate(60);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Compute the sibling worktree dir for a repo + branch. `myproj` +
/// `agentum/feat-foo` → `<parent>/myproj-worktrees/agentum-feat-foo`.
/// The second path component is already slugified (slashes → dashes)
/// because git allows `/` in branch names but most filesystems get
/// confused about them when they appear in `git worktree add <path>`.
fn worktree_dir_for(repo: &Path, branch: &str) -> PathBuf {
    let repo_name = repo
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo.parent().unwrap_or(Path::new("."));
    let safe_branch = branch.replace('/', "-");
    parent
        .join(format!("{}-worktrees", repo_name))
        .join(safe_branch)
}

/// Create a new worktree off `base_ref` checked out to a fresh branch
/// `branch_name` (slugified if missing). Returns the resolved details so
/// the caller can persist them on the Session row.
///
/// Errors:
///   * [`GitError::NotARepo`] if `repo` isn't inside a git working tree
///   * [`GitError::PathExists`] if the computed worktree dir already
///     exists (we never overwrite — that would silently break a
///     running session)
///   * [`GitError::BranchExists`] if the branch already exists in the
///     repo (the user is expected to pick a fresh name)
///   * [`GitError::CommandFailed`] on any other `git` non-zero exit
pub async fn create_worktree(
    repo: &Path,
    session_name: &str,
    requested_branch: Option<&str>,
    base_ref: Option<&str>,
) -> Result<ResolvedWorktree, GitError> {
    if !is_git_repo(repo).await {
        return Err(GitError::NotARepo(repo.to_path_buf()));
    }

    let base = base_ref.unwrap_or("HEAD").to_string();

    let branch = match requested_branch {
        Some(b) if !b.trim().is_empty() => b.trim().to_string(),
        _ => format!("agentum/{}", slugify_branch(session_name)),
    };

    let target_dir = worktree_dir_for(repo, &branch);
    if target_dir.exists() {
        return Err(GitError::PathExists(target_dir));
    }

    // -b fails fast if the branch already exists, which is the
    // semantic we want: collide → tell the user to pick another name
    // rather than silently sharing a branch between sessions.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b"])
        .arg(&branch)
        .arg(&target_dir)
        .arg(&base)
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("already exists") && stderr.contains("branch") {
            return Err(GitError::BranchExists(branch));
        }
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    Ok(ResolvedWorktree {
        path: target_dir,
        branch,
        base_ref: base,
    })
}

/// Recreate a worktree directory at a KNOWN path whose tree went missing
/// (pruned out-of-band, removed by hand, or a registry row that outlived
/// its checkout). Unlike [`create_worktree`], the caller already knows
/// `target` — we just need the directory back on disk so git/terminal ops
/// that open it stop failing.
///
/// Strategy: prune git's stale admin record, then try to re-attach the
/// existing branch named after the directory (the desktop's default); if
/// that branch is gone, fall back to creating a fresh one off `HEAD`. The
/// `.claude/worktrees` parent is created first since `git worktree add`
/// only makes the leaf.
pub async fn recreate_worktree(repo: &Path, target: &Path) -> Result<(), GitError> {
    if !is_git_repo(repo).await {
        return Err(GitError::NotARepo(repo.to_path_buf()));
    }
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| GitError::CommandFailed("worktree path has no final segment".into()))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Drop any stale "<target> is registered" bookkeeping so `worktree add`
    // doesn't refuse with "already registered" for a tree that's gone.
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "prune"])
        .output()
        .await;

    // Re-attach the existing branch (common case: the branch survived, only
    // the directory vanished).
    let attach = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add"])
        .arg(target)
        .arg(&name)
        .output()
        .await?;
    if attach.status.success() {
        return Ok(());
    }

    // Branch is gone too → create a fresh one off HEAD so the workspace at
    // least opens. The original branch (if any) still exists in the repo.
    let created = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b"])
        .arg(&name)
        .arg(target)
        .output()
        .await?;
    if created.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&created.stderr);
    Err(GitError::CommandFailed(stderr.into_owned()))
}

/// Tear down a worktree. Pass `force = true` to abandon uncommitted
/// changes (matches `git worktree remove --force`); otherwise git
/// refuses when the worktree has dirty files.
///
/// Also deletes the branch if it has no commits unique to it
/// (`git branch -d` semantics) — silently swallows the error
/// otherwise, since the worktree directory is gone and that's the
/// disk-cost surface the user cares about.
pub async fn prune_worktree(
    repo: &Path,
    worktree_path: &Path,
    branch: Option<&str>,
    force: bool,
) -> Result<(), GitError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(["worktree", "remove"]);
    if force {
        cmd.arg("--force");
    }
    cmd.arg(worktree_path);

    let out = cmd.output().await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    if let Some(b) = branch {
        // -d (not -D) → leaves branches that have unique commits in
        // place. The user can prune those manually if they want.
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["branch", "-d", b])
            .output()
            .await;
    }

    Ok(())
}

/// Tiny `git status --porcelain` summary used by the prune preflight
/// (so we can warn before deleting a worktree with unsaved work).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct WorktreeStatus {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
}

impl WorktreeStatus {
    pub fn is_clean(&self) -> bool {
        self.staged == 0 && self.unstaged == 0 && self.untracked == 0
    }
}

pub async fn worktree_status(path: &Path) -> Result<WorktreeStatus, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .await?;

    if !out.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }

    let mut s = WorktreeStatus::default();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 2 {
            continue;
        }
        let x = line.as_bytes()[0];
        let y = line.as_bytes()[1];
        if x == b'?' && y == b'?' {
            s.untracked += 1;
        } else {
            if x != b' ' {
                s.staged += 1;
            }
            if y != b' ' {
                s.unstaged += 1;
            }
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_handles_common_session_names() {
        assert_eq!(slugify_branch("My Cool Session"), "my-cool-session");
        assert_eq!(slugify_branch("hello world!!"), "hello-world");
        assert_eq!(slugify_branch("---trim---me---"), "trim-me");
        assert_eq!(slugify_branch(""), "session");
        let long = "a".repeat(80);
        assert!(slugify_branch(&long).len() <= 60);
    }

    #[test]
    fn worktree_dir_is_sibling_of_repo() {
        let p = worktree_dir_for(Path::new("/home/u/myproj"), "agentum/feat-foo");
        assert_eq!(
            p,
            PathBuf::from("/home/u/myproj-worktrees/agentum-feat-foo")
        );
    }
}
