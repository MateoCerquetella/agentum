//! `GET .../git/commit-compare` and `.../branch-compare` — diff summaries between
//! two refs, plus the name-status parser shared by both.
use super::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitCompareSummary {
    commit_oid: String,
    parent_oid: Option<String>,
    compare_ref: String,
    base_ref: String,
    changed_files: u32,
    /// `ready|invalid-commit|error`.
    status: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommitCompareResp {
    summary: CommitCompareSummary,
    entries: Vec<BranchChangeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitCompareQuery {
    commit: String,
}

/// `GET /api/sessions/{id}/git/commit-compare?commit=<oid>` — the diff a single
/// commit introduced (commit vs its first parent), reusing the name-status +
/// numstat parsing. A root commit (no parent) diffs against the empty tree.
pub(crate) async fn commit_compare(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<CommitCompareQuery>,
) -> Result<Json<CommitCompareResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let commit = q.commit.trim().to_string();
    let commit_oid = match run_git(
        &host,
        &cwd,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .await
    {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return Ok(Json(CommitCompareResp {
                summary: CommitCompareSummary {
                    commit_oid: commit.clone(),
                    parent_oid: None,
                    compare_ref: commit.clone(),
                    base_ref: String::new(),
                    changed_files: 0,
                    status: "invalid-commit".into(),
                },
                entries: Vec::new(),
            }));
        }
    };
    // First parent, if any. Root commits diff against the empty-tree object.
    let parent_oid = run_git(
        &host,
        &cwd,
        &["rev-parse", "--verify", &format!("{commit_oid}^")],
    )
    .await
    .ok()
    .map(|s| s.trim().to_string());
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let base = parent_oid.clone().unwrap_or_else(|| EMPTY_TREE.to_string());

    let range = format!("{base}..{commit_oid}");
    let name_status = run_git(&host, &cwd, &["diff", "--name-status", "-M", "-z", &range]).await?;
    let mut entries = parse_name_status_z(name_status.as_bytes());
    if let Ok(numstat) = run_git(&host, &cwd, &["diff", "--numstat", "-z", &range]).await {
        for rec in numstat.split('\0').filter(|r| !r.is_empty()) {
            let mut cols = rec.splitn(3, '\t');
            let added = cols.next().and_then(|s| s.parse::<u32>().ok());
            let removed = cols.next().and_then(|s| s.parse::<u32>().ok());
            if let Some(path) = cols.next() {
                if let Some(e) = entries.iter_mut().find(|e| e.path == path) {
                    e.added = added;
                    e.removed = removed;
                }
            }
        }
    }

    Ok(Json(CommitCompareResp {
        summary: CommitCompareSummary {
            changed_files: entries.len() as u32,
            commit_oid: commit_oid.clone(),
            parent_oid,
            compare_ref: commit_oid,
            base_ref: base,
            status: "ready".into(),
        },
        entries,
    }))
}

/// One changed path between two refs. Mirrors the desktop's
/// `GitBranchChangeEntry` (camelCase on the wire).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct BranchChangeEntry {
    path: String,
    /// `modified|added|deleted|renamed|copied`.
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchCompareSummary {
    base_ref: String,
    base_oid: Option<String>,
    compare_ref: String,
    head_oid: Option<String>,
    merge_base: Option<String>,
    changed_files: u32,
    commits_ahead: Option<u32>,
    /// `ready|invalid-base|unborn-head|no-merge-base|error`.
    status: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct BranchCompareResp {
    summary: BranchCompareSummary,
    entries: Vec<BranchChangeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BranchCompareQuery {
    base: String,
}

/// Map a `git diff --name-status` letter to the desktop status vocabulary.
fn map_change_status(letter: u8) -> &'static str {
    match letter {
        b'A' => "added",
        b'D' => "deleted",
        b'R' => "renamed",
        b'C' => "copied",
        _ => "modified", // M, T, and anything else read as a content change.
    }
}

/// Parse `git diff --name-status -M -z <range>` (NUL-delimited) into entries.
/// Rename/copy records are `R<score>\0<old>\0<new>`; others are `X\0<path>`.
/// Numstat (added/removed) is merged in separately by the caller.
fn parse_name_status_z(bytes: &[u8]) -> Vec<BranchChangeEntry> {
    let mut out = Vec::new();
    let mut it = bytes.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(code) = it.next() {
        let letter = code[0];
        let status = map_change_status(letter);
        if letter == b'R' || letter == b'C' {
            let old = it.next().map(|b| String::from_utf8_lossy(b).into_owned());
            let Some(new) = it.next() else { break };
            out.push(BranchChangeEntry {
                path: String::from_utf8_lossy(new).into_owned(),
                status: status.to_string(),
                old_path: old,
                added: None,
                removed: None,
            });
        } else {
            let Some(path) = it.next() else { break };
            out.push(BranchChangeEntry {
                path: String::from_utf8_lossy(path).into_owned(),
                status: status.to_string(),
                old_path: None,
                added: None,
                removed: None,
            });
        }
    }
    out
}

/// `GET /api/sessions/{id}/git/branch-compare?base=<ref>` — diff the worktree's
/// HEAD against `base` (3-dot, from the merge-base), with per-file add/remove counts.
pub(crate) async fn branch_compare(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BranchCompareQuery>,
) -> Result<Json<BranchCompareResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let base = q.base.trim().to_string();
    let compare_ref = run_git(&host, &cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".into());
    let head_oid = run_git(&host, &cwd, &["rev-parse", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string());
    let base_oid = run_git(&host, &cwd, &["rev-parse", &base])
        .await
        .ok()
        .map(|s| s.trim().to_string());

    let err = |status: &str| BranchCompareResp {
        summary: BranchCompareSummary {
            base_ref: base.clone(),
            base_oid: base_oid.clone(),
            compare_ref: compare_ref.clone(),
            head_oid: head_oid.clone(),
            merge_base: None,
            changed_files: 0,
            commits_ahead: None,
            status: status.to_string(),
        },
        entries: Vec::new(),
    };
    if base_oid.is_none() {
        return Ok(Json(err("invalid-base")));
    }
    if head_oid.is_none() {
        return Ok(Json(err("unborn-head")));
    }
    let merge_base = match run_git(&host, &cwd, &["merge-base", &base, "HEAD"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => return Ok(Json(err("no-merge-base"))),
    };

    let range = format!("{merge_base}..HEAD");
    let name_status = run_git(&host, &cwd, &["diff", "--name-status", "-M", "-z", &range]).await?;
    let mut entries = parse_name_status_z(name_status.as_bytes());

    // Merge numstat (added/removed). `--numstat -z` gives `<add>\t<del>\t<path>\0`
    // (binary files show `-`); index by path so rename targets line up.
    if let Ok(numstat) = run_git(&host, &cwd, &["diff", "--numstat", "-z", &range]).await {
        for rec in numstat.split('\0').filter(|r| !r.is_empty()) {
            let mut cols = rec.splitn(3, '\t');
            let added = cols.next().and_then(|s| s.parse::<u32>().ok());
            let removed = cols.next().and_then(|s| s.parse::<u32>().ok());
            if let Some(path) = cols.next() {
                if let Some(e) = entries.iter_mut().find(|e| e.path == path) {
                    e.added = added;
                    e.removed = removed;
                }
            }
        }
    }

    let commits_ahead = run_git(&host, &cwd, &["rev-list", "--count", &range])
        .await
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    Ok(Json(BranchCompareResp {
        summary: BranchCompareSummary {
            base_ref: base,
            base_oid,
            compare_ref,
            head_oid,
            merge_base: Some(merge_base),
            changed_files: entries.len() as u32,
            commits_ahead,
            status: "ready".into(),
        },
        entries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_status_handles_renames_and_plain_changes() {
        // `M\0a.rs\0` (modify), `A\0b.rs\0` (add), `R100\0old.rs\0new.rs\0` (rename).
        let input = b"M\0a.rs\0A\0b.rs\0R100\0old.rs\0new.rs\0";
        let entries = parse_name_status_z(input);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].status, "modified");
        assert_eq!(entries[0].path, "a.rs");
        assert_eq!(entries[1].status, "added");
        assert_eq!(entries[2].status, "renamed");
        assert_eq!(entries[2].path, "new.rs");
        assert_eq!(entries[2].old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn map_change_status_covers_git_letters() {
        assert_eq!(map_change_status(b'A'), "added");
        assert_eq!(map_change_status(b'D'), "deleted");
        assert_eq!(map_change_status(b'R'), "renamed");
        assert_eq!(map_change_status(b'C'), "copied");
        assert_eq!(map_change_status(b'M'), "modified");
        assert_eq!(map_change_status(b'T'), "modified");
    }
}
