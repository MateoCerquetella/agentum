use base64::Engine as _;
use git2::{BranchType, Cred, PushOptions, RemoteCallbacks, Repository};
use serde::Serialize;
use serde_json::{Map, Value};

// Mirrors GitStatusResult/GitUncommittedEntry/GitUpstreamStatus in
// orca/src/shared/git-status-types.ts. A file can appear in both staging areas,
// so one git2 entry can yield multiple GitStatusEntry rows (one per area).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    path: String,
    status: String,
    area: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitUpstreamStatus {
    has_upstream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_name: Option<String>,
    ahead: u64,
    behind: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusResult {
    entries: Vec<GitStatusEntry>,
    conflict_operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_status: Option<GitUpstreamStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignored_paths: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct GitBranchInfo {
    pub name: String,
    pub is_head: bool,
}

#[derive(Debug, Serialize)]
pub struct GitCommitEntry {
    pub id: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn repo_from_path(path: &str) -> Result<Repository, String> {
    Repository::discover(path).map_err(map_err)
}

fn status_entry(path: &str, status: &str, area: &str) -> GitStatusEntry {
    GitStatusEntry {
        path: path.to_string(),
        status: status.to_string(),
        area: area.to_string(),
        old_path: None,
    }
}

// Ahead/behind vs. the current branch's upstream, if one is configured.
fn upstream_status(repo: &Repository, head: Option<&git2::Reference>) -> Option<GitUpstreamStatus> {
    let head = head?;
    let branch_name = head.shorthand()?;
    let local = repo.find_branch(branch_name, BranchType::Local).ok()?;
    match local.upstream() {
        Ok(upstream) => {
            let upstream_name = upstream.name().ok().flatten().map(str::to_string);
            let (ahead, behind) = match (head.target(), upstream.get().target()) {
                (Some(local_oid), Some(upstream_oid)) => repo
                    .graph_ahead_behind(local_oid, upstream_oid)
                    .unwrap_or((0, 0)),
                _ => (0, 0),
            };
            Some(GitUpstreamStatus {
                has_upstream: true,
                upstream_name,
                ahead: ahead as u64,
                behind: behind as u64,
            })
        }
        Err(_) => Some(GitUpstreamStatus {
            has_upstream: false,
            upstream_name: None,
            ahead: 0,
            behind: 0,
        }),
    }
}

#[tauri::command]
pub async fn git_status(
    worktree_path: String,
    connection_id: Option<String>,
    include_ignored: Option<bool>,
) -> Result<GitStatusResult, String> {
    let _ = connection_id; // SSH transport not ported yet; local only for now.
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&worktree_path)?;
        let include_ignored = include_ignored.unwrap_or(false);

        let mut options = git2::StatusOptions::new();
        options
            .include_untracked(true)
            .include_ignored(include_ignored)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);
        let statuses = repo.statuses(Some(&mut options)).map_err(map_err)?;

        let mut entries = Vec::new();
        let mut ignored_paths = Vec::new();
        for entry in statuses.iter() {
            let flags = entry.status();
            let path = entry.path().map(str::to_string).unwrap_or_default();
            if flags.is_ignored() {
                ignored_paths.push(path);
                continue;
            }
            // Index (staged) area.
            if flags.is_index_new() {
                entries.push(status_entry(&path, "added", "staged"));
            }
            if flags.is_index_modified() || flags.is_index_typechange() {
                entries.push(status_entry(&path, "modified", "staged"));
            }
            if flags.is_index_deleted() {
                entries.push(status_entry(&path, "deleted", "staged"));
            }
            if flags.is_index_renamed() {
                entries.push(status_entry(&path, "renamed", "staged"));
            }
            // Worktree (unstaged) area.
            if flags.is_wt_modified() || flags.is_wt_typechange() {
                entries.push(status_entry(&path, "modified", "unstaged"));
            }
            if flags.is_wt_deleted() {
                entries.push(status_entry(&path, "deleted", "unstaged"));
            }
            if flags.is_wt_renamed() {
                entries.push(status_entry(&path, "renamed", "unstaged"));
            }
            if flags.is_wt_new() {
                entries.push(status_entry(&path, "untracked", "untracked"));
            }
            if flags.is_conflicted() {
                entries.push(status_entry(&path, "modified", "unstaged"));
            }
        }

        let conflict_operation = repo_conflict_operation(&repo);

        let head_ref = repo.head().ok();
        let head = head_ref
            .as_ref()
            .and_then(|reference| reference.target())
            .map(|oid| oid.to_string());
        let branch = head_ref
            .as_ref()
            .and_then(|reference| reference.shorthand().map(str::to_string));
        let upstream = upstream_status(&repo, head_ref.as_ref());

        Ok(GitStatusResult {
            entries,
            conflict_operation,
            head,
            branch,
            upstream_status: upstream,
            ignored_paths: include_ignored.then_some(ignored_paths),
        })
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn git_branch(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&path)?;
        let head = repo.head().map_err(map_err)?;
        Ok(head.shorthand().unwrap_or("HEAD").to_string())
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn git_branches(path: String) -> Result<Vec<GitBranchInfo>, String> {
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&path)?;
        let mut branches = Vec::new();
        let iterator = repo.branches(Some(BranchType::Local)).map_err(map_err)?;

        for branch in iterator {
            let (branch, _) = branch.map_err(map_err)?;
            branches.push(GitBranchInfo {
                name: branch
                    .name()
                    .map_err(map_err)?
                    .unwrap_or_default()
                    .to_string(),
                is_head: branch.is_head(),
            });
        }

        Ok(branches)
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn git_log(path: String, limit: usize) -> Result<Vec<GitCommitEntry>, String> {
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&path)?;
        let mut revwalk = repo.revwalk().map_err(map_err)?;
        revwalk.push_head().map_err(map_err)?;

        let mut commits = Vec::new();
        for oid in revwalk.take(limit) {
            let oid = oid.map_err(map_err)?;
            let commit = repo.find_commit(oid).map_err(map_err)?;
            let author = commit.author();
            commits.push(GitCommitEntry {
                id: commit.id().to_string(),
                summary: commit.summary().unwrap_or_default().to_string(),
                author: author.name().unwrap_or_default().to_string(),
                email: author.email().unwrap_or_default().to_string(),
                timestamp: commit.time().seconds(),
            });
        }

        Ok(commits)
    })
    .await
    .map_err(map_err)?
}

// Returns a blob's bytes at `spec` (e.g. "HEAD:path" or ":path" for the index),
// or None if the file is absent there (e.g. newly added has no HEAD blob).
async fn git_show(worktree_path: &str, spec: &str) -> Option<Vec<u8>> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["show", spec])
        .output()
        .await
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn guess_mime(file_path: &str) -> Option<&'static str> {
    let lower = file_path.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".gif") {
        Some("image/gif")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else if lower.ends_with(".svg") {
        Some("image/svg+xml")
    } else if lower.ends_with(".pdf") {
        Some("application/pdf")
    } else {
        None
    }
}

// GitDiffResult is a before/after content pair, not a patch — the renderer diffs
// the two contents itself. Text → utf8 strings; binary → base64.
#[tauri::command]
pub async fn git_diff(
    worktree_path: String,
    file_path: String,
    staged: bool,
    compare_against_head: Option<bool>,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id; // SSH transport not ported yet.
    let against_head = compare_against_head.unwrap_or(false);

    let original_spec = if staged || against_head {
        format!("HEAD:{file_path}")
    } else {
        format!(":{file_path}")
    };
    let original = git_show(&worktree_path, &original_spec)
        .await
        .unwrap_or_default();
    let modified = if staged {
        git_show(&worktree_path, &format!(":{file_path}"))
            .await
            .unwrap_or_default()
    } else {
        let full = std::path::Path::new(&worktree_path).join(&file_path);
        tokio::fs::read(&full).await.unwrap_or_default()
    };

    Ok(diff_result_value(&file_path, &original, &modified))
}

async fn run_git_void(worktree_path: &str, args: &[&str]) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(args)
        .output()
        .await
        .map_err(map_err)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[tauri::command]
pub async fn git_fetch(worktree_path: String, connection_id: Option<String>) -> Result<(), String> {
    let _ = connection_id;
    run_git_void(&worktree_path, &["fetch"]).await
}

#[tauri::command]
pub async fn git_pull(worktree_path: String, connection_id: Option<String>) -> Result<(), String> {
    let _ = connection_id;
    run_git_void(&worktree_path, &["pull"]).await
}

#[tauri::command]
pub async fn git_fast_forward(
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    // Fast-forward the current branch to its configured upstream only.
    run_git_void(&worktree_path, &["merge", "--ff-only", "@{upstream}"]).await
}

#[tauri::command]
pub async fn git_abort_merge(
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    run_git_void(&worktree_path, &["merge", "--abort"]).await
}

#[tauri::command]
pub async fn git_abort_rebase(
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    run_git_void(&worktree_path, &["rebase", "--abort"]).await
}

#[tauri::command]
pub async fn git_check_ignored(
    worktree_path: String,
    paths: Vec<String>,
    connection_id: Option<String>,
) -> Result<Vec<String>, String> {
    let _ = connection_id;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(&worktree_path)
        .arg("check-ignore")
        .arg("--");
    for path in &paths {
        command.arg(path);
    }
    let output = command.output().await.map_err(map_err)?;
    // check-ignore exits 0 (some ignored), 1 (none ignored), >1 (real error).
    if matches!(output.status.code(), Some(code) if code > 1) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

#[tauri::command]
pub async fn git_upstream_status(
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<GitUpstreamStatus, String> {
    let _ = connection_id;
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&worktree_path)?;
        let head = repo.head().ok();
        Ok(upstream_status(&repo, head.as_ref()).unwrap_or(GitUpstreamStatus {
            has_upstream: false,
            upstream_name: None,
            ahead: 0,
            behind: 0,
        }))
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn git_history(
    worktree_path: String,
    connection_id: Option<String>,
    limit: Option<usize>,
    base_ref: Option<String>,
) -> Result<Value, String> {
    let _ = (connection_id, base_ref);
    let limit = limit.unwrap_or(50);
    // \x1f field sep, \x1e record sep. Fetch one extra to detect hasMore.
    let format = "%H\u{1f}%P\u{1f}%s\u{1f}%B\u{1f}%an\u{1f}%ae\u{1f}%at\u{1e}";
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&worktree_path)
        .args([
            "log",
            &format!("--max-count={}", limit + 1),
            &format!("--pretty=format:{format}"),
        ])
        .output()
        .await
        .map_err(map_err)?;

    let mut items: Vec<Value> = Vec::new();
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for record in stdout.split('\u{1e}') {
            let record = record.trim_matches(['\n', '\r']);
            if record.is_empty() {
                continue;
            }
            let fields: Vec<&str> = record.split('\u{1f}').collect();
            if fields.len() < 7 {
                continue;
            }
            let parent_ids: Vec<String> =
                fields[1].split_whitespace().map(str::to_string).collect();
            items.push(serde_json::json!({
                "id": fields[0],
                "parentIds": parent_ids,
                "subject": fields[2],
                "message": fields[3],
                "displayId": fields[0].chars().take(8).collect::<String>(),
                "author": fields[4],
                "authorEmail": fields[5],
                "timestamp": fields[6].trim().parse::<i64>().ok(),
            }));
        }
    }
    let has_more = items.len() > limit;
    items.truncate(limit);

    // Current-ref + incoming/outgoing via git2.
    let (incoming, outgoing, current_ref) = tokio::task::spawn_blocking(move || {
        let mut incoming = false;
        let mut outgoing = false;
        let mut current_ref = Value::Null;
        if let Ok(repo) = repo_from_path(&worktree_path) {
            let head = repo.head().ok();
            if let Some(status) = head.as_ref().and_then(|head| upstream_status(&repo, Some(head)))
            {
                incoming = status.behind > 0;
                outgoing = status.ahead > 0;
            }
            if let Some(head) = head.as_ref() {
                if let (Some(oid), Some(name)) = (head.target(), head.shorthand()) {
                    current_ref = serde_json::json!({ "id": oid.to_string(), "name": name });
                }
            }
        }
        (incoming, outgoing, current_ref)
    })
    .await
    .map_err(map_err)?;

    let mut result = Map::new();
    result.insert("items".into(), Value::Array(items));
    result.insert("hasIncomingChanges".into(), incoming.into());
    result.insert("hasOutgoingChanges".into(), outgoing.into());
    result.insert("hasMore".into(), has_more.into());
    result.insert("limit".into(), (limit as u64).into());
    if !current_ref.is_null() {
        result.insert("currentRef".into(), current_ref);
    }
    Ok(Value::Object(result))
}

// Builds a GitDiffResult (content-pair) from before/after bytes. Shared by
// git_diff, git_commit_diff, git_branch_diff.
fn diff_result_value(file_path: &str, original: &[u8], modified: &[u8]) -> Value {
    let original_is_binary = is_binary(original);
    let modified_is_binary = is_binary(modified);
    if original_is_binary || modified_is_binary {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut object = Map::new();
        object.insert("kind".into(), "binary".into());
        object.insert("originalContent".into(), engine.encode(original).into());
        object.insert("modifiedContent".into(), engine.encode(modified).into());
        object.insert("originalIsBinary".into(), original_is_binary.into());
        object.insert("modifiedIsBinary".into(), modified_is_binary.into());
        if let Some(mime) = guess_mime(file_path) {
            object.insert("isImage".into(), mime.starts_with("image/").into());
            object.insert("mimeType".into(), mime.into());
        }
        Value::Object(object)
    } else {
        serde_json::json!({
            "kind": "text",
            "originalContent": String::from_utf8_lossy(original),
            "modifiedContent": String::from_utf8_lossy(modified),
            "originalIsBinary": false,
            "modifiedIsBinary": false,
        })
    }
}

fn repo_conflict_operation(repo: &Repository) -> String {
    match repo.state() {
        git2::RepositoryState::Merge => "merge",
        git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence => {
            "cherry-pick"
        }
        git2::RepositoryState::Rebase
        | git2::RepositoryState::RebaseInteractive
        | git2::RepositoryState::RebaseMerge
        | git2::RepositoryState::ApplyMailbox
        | git2::RepositoryState::ApplyMailboxOrRebase => "rebase",
        _ => "unknown",
    }
    .to_string()
}

#[tauri::command]
pub async fn git_commit_diff(
    worktree_path: String,
    commit_oid: String,
    parent_oid: Option<String>,
    file_path: String,
    old_path: Option<String>,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id;
    let old = old_path.unwrap_or_else(|| file_path.clone());
    // No parent (root commit) → original is empty.
    let original = match parent_oid {
        Some(parent) => git_show(&worktree_path, &format!("{parent}:{old}")).await,
        None => None,
    }
    .unwrap_or_default();
    let modified = git_show(&worktree_path, &format!("{commit_oid}:{file_path}"))
        .await
        .unwrap_or_default();
    Ok(diff_result_value(&file_path, &original, &modified))
}

#[tauri::command]
pub async fn git_branch_diff(
    worktree_path: String,
    compare: Value,
    file_path: String,
    old_path: Option<String>,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id;
    let get = |key: &str| {
        compare
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let merge_base = get("mergeBase");
    let base = if merge_base.is_empty() {
        get("baseOid")
    } else {
        merge_base
    };
    let head_oid = get("headOid");
    let old = old_path.unwrap_or_else(|| file_path.clone());
    let original = git_show(&worktree_path, &format!("{base}:{old}"))
        .await
        .unwrap_or_default();
    let modified = git_show(&worktree_path, &format!("{head_oid}:{file_path}"))
        .await
        .unwrap_or_default();
    Ok(diff_result_value(&file_path, &original, &modified))
}

#[tauri::command]
pub async fn git_conflict_operation(
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<String, String> {
    let _ = connection_id;
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&worktree_path)?;
        Ok(repo_conflict_operation(&repo))
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn git_rebase_from_base(
    worktree_path: String,
    base_ref: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    run_git_void(&worktree_path, &["rebase", &base_ref]).await
}

async fn git_stdout(worktree_path: &str, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(args)
        .output()
        .await
        .map_err(map_err)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

// Resolves a rev to its oid, or None when it does not exist (e.g. unborn HEAD).
async fn rev_parse(worktree_path: &str, rev: &str) -> Option<String> {
    let out = git_stdout(worktree_path, &["rev-parse", "--verify", "-q", rev])
        .await
        .ok()?;
    let trimmed = out.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

// Parses `git diff --name-status -M` lines into GitBranchChangeEntry values.
fn parse_name_status(output: &str) -> Vec<Value> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let code = parts[0].chars().next().unwrap_or('M');
        let (status, path, old_path) = match code {
            'A' => ("added", parts[1], None),
            'D' => ("deleted", parts[1], None),
            'R' => ("renamed", *parts.get(2).unwrap_or(&parts[1]), Some(parts[1])),
            'C' => ("copied", *parts.get(2).unwrap_or(&parts[1]), Some(parts[1])),
            _ => ("modified", parts[1], None),
        };
        let mut entry = Map::new();
        entry.insert("path".into(), path.into());
        entry.insert("status".into(), status.into());
        if let Some(old) = old_path {
            entry.insert("oldPath".into(), old.into());
        }
        entries.push(Value::Object(entry));
    }
    entries
}

#[tauri::command]
pub async fn git_branch_compare(
    worktree_path: String,
    base_ref: String,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id;
    let head_oid = rev_parse(&worktree_path, "HEAD").await;
    let base_oid = rev_parse(&worktree_path, &base_ref).await;

    let mut summary = Map::new();
    summary.insert("baseRef".into(), base_ref.clone().into());
    summary.insert("compareRef".into(), "HEAD".into());
    summary.insert(
        "baseOid".into(),
        base_oid.clone().map(Value::from).unwrap_or(Value::Null),
    );
    summary.insert(
        "headOid".into(),
        head_oid.clone().map(Value::from).unwrap_or(Value::Null),
    );

    let early_status = if base_oid.is_none() {
        Some("invalid-base")
    } else if head_oid.is_none() {
        Some("unborn-head")
    } else {
        None
    };
    if let Some(status) = early_status {
        summary.insert("mergeBase".into(), Value::Null);
        summary.insert("changedFiles".into(), 0.into());
        summary.insert("status".into(), status.into());
        return Ok(serde_json::json!({ "summary": Value::Object(summary), "entries": [] }));
    }

    let merge_base = git_stdout(&worktree_path, &["merge-base", &base_ref, "HEAD"])
        .await
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|out| !out.is_empty());
    summary.insert(
        "mergeBase".into(),
        merge_base.map(Value::from).unwrap_or(Value::Null),
    );

    let name_status = git_stdout(
        &worktree_path,
        &["diff", "--name-status", "-M", &format!("{base_ref}...HEAD")],
    )
    .await
    .unwrap_or_default();
    let entries = parse_name_status(&name_status);
    let commits_ahead = git_stdout(
        &worktree_path,
        &["rev-list", "--count", &format!("{base_ref}..HEAD")],
    )
    .await
    .ok()
    .and_then(|out| out.trim().parse::<u64>().ok());

    summary.insert("changedFiles".into(), (entries.len() as u64).into());
    if let Some(ahead) = commits_ahead {
        summary.insert("commitsAhead".into(), ahead.into());
    }
    summary.insert("status".into(), "ready".into());
    Ok(serde_json::json!({ "summary": Value::Object(summary), "entries": entries }))
}

#[tauri::command]
pub async fn git_commit_compare(
    worktree_path: String,
    commit_id: String,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id;
    let mut summary = Map::new();
    summary.insert("compareRef".into(), commit_id.clone().into());
    summary.insert("baseRef".into(), format!("{commit_id}^").into());

    let Some(commit_oid) = rev_parse(&worktree_path, &commit_id).await else {
        summary.insert("commitOid".into(), Value::Null);
        summary.insert("parentOid".into(), Value::Null);
        summary.insert("changedFiles".into(), 0.into());
        summary.insert("status".into(), "invalid-commit".into());
        return Ok(serde_json::json!({ "summary": Value::Object(summary), "entries": [] }));
    };

    summary.insert("commitOid".into(), commit_oid.clone().into());
    let parent = rev_parse(&worktree_path, &format!("{commit_id}^")).await;
    summary.insert(
        "parentOid".into(),
        parent.map(Value::from).unwrap_or(Value::Null),
    );
    // --format= suppresses the commit header; root commits list all files as added.
    let name_status = git_stdout(
        &worktree_path,
        &["show", "--name-status", "-M", "--format=", &commit_oid],
    )
    .await
    .unwrap_or_default();
    let entries = parse_name_status(name_status.trim());
    summary.insert("changedFiles".into(), (entries.len() as u64).into());
    summary.insert("status".into(), "ready".into());
    Ok(serde_json::json!({ "summary": Value::Object(summary), "entries": entries }))
}

// Converts a git remote URL (scp-like, ssh://, or http(s)://) to (web_base, host).
fn git_url_to_web_base(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return Some((format!("https://{host}/{path}"), host.to_string()));
        }
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        if let Some((host, path)) = rest.split_once('/') {
            return Some((format!("https://{host}/{path}"), host.to_string()));
        }
    }
    for prefix in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
            if let Some((host, path)) = rest.split_once('/') {
                return Some((format!("https://{host}/{path}"), host.to_string()));
            }
        }
    }
    None
}

fn build_file_url(web_base: &str, host: &str, reference: &str, path: &str, line: i64) -> String {
    let host = host.to_lowercase();
    if host.contains("gitlab") {
        format!("{web_base}/-/blob/{reference}/{path}#L{line}")
    } else if host.contains("bitbucket") {
        format!("{web_base}/src/{reference}/{path}#lines-{line}")
    } else {
        format!("{web_base}/blob/{reference}/{path}#L{line}")
    }
}

#[tauri::command]
pub async fn git_remote_file_url(
    worktree_path: String,
    relative_path: String,
    line: i64,
    connection_id: Option<String>,
) -> Result<Option<String>, String> {
    let _ = connection_id;
    let remote = match git_stdout(&worktree_path, &["remote", "get-url", "origin"]).await {
        Ok(out) => out.trim().to_string(),
        Err(_) => return Ok(None),
    };
    let Some((web_base, host)) = git_url_to_web_base(&remote) else {
        return Ok(None);
    };
    // Prefer the branch name; fall back to the commit oid for detached HEAD.
    let branch = git_stdout(&worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .map(|out| out.trim().to_string());
    let reference = match branch {
        Some(name) if !name.is_empty() && name != "HEAD" => name,
        _ => rev_parse(&worktree_path, "HEAD")
            .await
            .unwrap_or_else(|| "HEAD".to_string()),
    };
    Ok(Some(build_file_url(
        &web_base,
        &host,
        &reference,
        &relative_path,
        line,
    )))
}

// LLM-backed commit-message / PR-field generation needs the agent runtime, which
// isn't ported. Return the contract's failure variant; cancels are no-ops.
#[tauri::command]
pub fn git_generate_commit_message() -> Value {
    serde_json::json!({
        "success": false,
        "error": "Commit-message generation requires the agent runtime, which isn't available yet."
    })
}

#[tauri::command]
pub fn git_generate_pull_request_fields() -> Value {
    serde_json::json!({
        "success": false,
        "error": "PR-field generation requires the agent runtime, which isn't available yet."
    })
}

#[tauri::command]
pub fn git_discover_commit_message_models() -> Value {
    serde_json::json!({ "success": false, "error": "No commit-message models available." })
}

#[tauri::command]
pub fn git_cancel_generate_commit_message() {}

#[tauri::command]
pub fn git_cancel_generate_pull_request_fields() {}

// Runs `git -C <worktree> <leading...> -- <paths...>` for path-scoped staging ops.
// CLI (not git2) so git config, attributes, and hooks behave as the user expects.
async fn git_pathspec(
    worktree_path: &str,
    leading: &[&str],
    file_paths: &[String],
) -> Result<(), String> {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(worktree_path).args(leading).arg("--");
    for path in file_paths {
        command.arg(path);
    }
    let output = command.output().await.map_err(map_err)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[tauri::command]
pub async fn git_stage(
    worktree_path: String,
    file_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id; // SSH transport not ported yet.
    git_pathspec(&worktree_path, &["add"], &[file_path]).await
}

#[tauri::command]
pub async fn git_bulk_stage(
    worktree_path: String,
    file_paths: Vec<String>,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    git_pathspec(&worktree_path, &["add"], &file_paths).await
}

#[tauri::command]
pub async fn git_unstage(
    worktree_path: String,
    file_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    git_pathspec(&worktree_path, &["reset", "-q", "HEAD"], &[file_path]).await
}

#[tauri::command]
pub async fn git_bulk_unstage(
    worktree_path: String,
    file_paths: Vec<String>,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    git_pathspec(&worktree_path, &["reset", "-q", "HEAD"], &file_paths).await
}

#[tauri::command]
pub async fn git_discard(
    worktree_path: String,
    file_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    // Discards tracked-file changes; untracked-file deletion is a later refinement.
    git_pathspec(&worktree_path, &["checkout"], &[file_path]).await
}

#[tauri::command]
pub async fn git_bulk_discard(
    worktree_path: String,
    file_paths: Vec<String>,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    git_pathspec(&worktree_path, &["checkout"], &file_paths).await
}

#[derive(Debug, Serialize)]
pub struct GitCommitResult {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[tauri::command]
pub async fn git_commit(
    worktree_path: String,
    message: String,
    connection_id: Option<String>,
) -> Result<GitCommitResult, String> {
    let _ = connection_id;
    // CLI commit so user.name/email, hooks, and signing config all apply.
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&worktree_path)
        .args(["commit", "-m", &message])
        .output()
        .await
        .map_err(map_err)?;
    if output.status.success() {
        Ok(GitCommitResult {
            success: true,
            error: None,
        })
    } else {
        Ok(GitCommitResult {
            success: false,
            error: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        })
    }
}

#[tauri::command]
pub async fn git_push(path: String, remote: String, branch: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let repo = repo_from_path(&path)?;
        let mut remote = repo.find_remote(&remote).map_err(map_err)?;
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_url, username_from_url, _allowed| {
            if let Some(username) = username_from_url {
                Cred::ssh_key_from_agent(username).or_else(|_| Cred::default())
            } else {
                Cred::default()
            }
        });

        let mut push_options = PushOptions::new();
        push_options.remote_callbacks(callbacks);

        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        remote
            .push(&[refspec.as_str()], Some(&mut push_options))
            .map_err(map_err)
    })
    .await
    .map_err(map_err)?
}
