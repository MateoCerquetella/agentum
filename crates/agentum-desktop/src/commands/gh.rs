use serde_json::{json, Value};

// The GitHub work-item list is served by shelling out to the authenticated `gh`
// CLI (the same tool the preflight check probes), which avoids a bespoke REST +
// GraphQL + token-store client in the desktop. The richer drawer surfaces
// (details, comments, checks, mutations) remain stubbed below and await that
// client; this file implements the list read so the task board stops failing.

// owner/repo = the last two path segments of the remote URL (scp-like or http(s)).
fn owner_repo_from_remote(remote: &str) -> Option<(String, String)> {
    let url = remote.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    let parts: Vec<&str> = url
        .split(['/', ':'])
        .filter(|part| !part.is_empty())
        .collect();
    (parts.len() >= 2).then(|| {
        (
            parts[parts.len() - 2].to_string(),
            parts[parts.len() - 1].to_string(),
        )
    })
}

// Resolve owner/repo from the repo's origin remote. None for folder-mode repos
// or anything without an origin (the caller treats that as "not a GitHub repo"
// and returns an empty, error-free envelope).
async fn resolve_owner_repo(repo_path: &str) -> Option<(String, String)> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    owner_repo_from_remote(&remote)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkItemKind {
    Issue,
    Pr,
}

// The renderer's search box carries GitHub search syntax ("is:issue is:open" for
// the Issues tab, "is:pr is:open" for PRs). Parse the `is:` qualifiers we map to
// gh flags and keep the rest as a free-text `--search` term.
struct ParsedQuery {
    kinds: Vec<WorkItemKind>,
    state: String,
    search: String,
}

fn parse_query(query: &str) -> ParsedQuery {
    let mut kinds: Vec<WorkItemKind> = Vec::new();
    let mut state = "open".to_string();
    let mut search_terms: Vec<String> = Vec::new();
    for token in query.split_whitespace() {
        match token.to_lowercase().as_str() {
            "is:issue" => {
                if !kinds.contains(&WorkItemKind::Issue) {
                    kinds.push(WorkItemKind::Issue);
                }
            }
            "is:pr" | "is:pull-request" => {
                if !kinds.contains(&WorkItemKind::Pr) {
                    kinds.push(WorkItemKind::Pr);
                }
            }
            "is:open" => state = "open".into(),
            "is:closed" => state = "closed".into(),
            "is:merged" => state = "merged".into(),
            // `is:draft` is a PR sub-state we derive from `isDraft`, not a gh
            // --state value; drop it so it doesn't leak into the search term.
            "is:draft" => {}
            _ => search_terms.push(token.to_string()),
        }
    }
    if kinds.is_empty() {
        // No kind qualifier (a bare text search) → cover both surfaces.
        kinds.push(WorkItemKind::Issue);
        kinds.push(WorkItemKind::Pr);
    }
    ParsedQuery {
        kinds,
        state,
        search: search_terms.join(" "),
    }
}

// Issues can't be "merged"; collapse that to "all" so the gh flag stays valid.
fn gh_state_arg(kind: WorkItemKind, state: &str) -> &'static str {
    match (kind, state) {
        (_, "closed") => "closed",
        (_, "all") => "all",
        (WorkItemKind::Pr, "merged") => "merged",
        (WorkItemKind::Issue, "merged") => "all",
        _ => "open",
    }
}

struct GhError {
    kind: &'static str,
    message: String,
}

// Map gh stderr onto the renderer's ClassifiedError union so the per-repo banner
// renders the right copy. "not_found" is treated as benign by the caller (e.g. a
// non-GitHub repo in a mixed selection), matching the existing GitLab path.
fn classify_gh_error(stderr: &str) -> GhError {
    let lower = stderr.to_lowercase();
    let kind = if lower.contains("could not resolve to a repository")
        || lower.contains("not found")
        || lower.contains("404")
    {
        "not_found"
    } else if lower.contains("issues are disabled") || lower.contains("has disabled issues") {
        "issues_disabled"
    } else if lower.contains("authentication")
        || lower.contains("gh auth login")
        || lower.contains("not logged in")
        || lower.contains("401")
        || lower.contains("403")
    {
        "permission_denied"
    } else if lower.contains("rate limit") {
        "rate_limited"
    } else if lower.contains("could not connect") || lower.contains("timeout") {
        "network_error"
    } else {
        "unknown"
    };
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("GitHub request failed.")
        .to_string();
    GhError { kind, message }
}

// gh emits one branch name per line (we pass `--jq '.[].name'`); trim and drop
// blank lines so a trailing newline or a paginated gap doesn't yield empty entries.
fn parse_branch_names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn label_names(item: &Value) -> Vec<String> {
    item.get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.get("name").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// gh's issue/PR list omits avatars, but GitHub serves a stable avatar at
// github.com/<login>.png — synthesize it so assignee chips render.
fn assignees(item: &Value) -> Vec<Value> {
    item.get("assignees")
        .and_then(Value::as_array)
        .map(|users| {
            users
                .iter()
                .filter_map(|user| {
                    let login = user.get("login").and_then(Value::as_str)?;
                    Some(json!({
                        "login": login,
                        "name": user.get("name").and_then(Value::as_str),
                        "avatarUrl": format!("https://github.com/{login}.png"),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn map_issue(item: &Value) -> Value {
    json!({
        "id": item.get("id").and_then(Value::as_str).unwrap_or_default(),
        "type": "issue",
        "number": item.get("number").and_then(Value::as_i64).unwrap_or_default(),
        "title": item.get("title").and_then(Value::as_str).unwrap_or_default(),
        "state": item.get("state").and_then(Value::as_str).unwrap_or("OPEN").to_lowercase(),
        "url": item.get("url").and_then(Value::as_str).unwrap_or_default(),
        "labels": label_names(item),
        "updatedAt": item.get("updatedAt").and_then(Value::as_str).unwrap_or_default(),
        "author": item.get("author").and_then(|a| a.get("login")).and_then(Value::as_str),
        "assignees": assignees(item),
    })
}

fn map_pr(item: &Value) -> Value {
    let is_draft = item
        .get("isDraft")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let raw_state = item.get("state").and_then(Value::as_str).unwrap_or("OPEN");
    // GitHubWorkItem.state encodes draft as its own variant; only an *open* PR
    // can be a draft, so don't let a draft flag mask a closed/merged state.
    let state = if is_draft && raw_state.eq_ignore_ascii_case("open") {
        "draft".to_string()
    } else {
        raw_state.to_lowercase()
    };
    let mut mapped = json!({
        "id": item.get("id").and_then(Value::as_str).unwrap_or_default(),
        "type": "pr",
        "number": item.get("number").and_then(Value::as_i64).unwrap_or_default(),
        "title": item.get("title").and_then(Value::as_str).unwrap_or_default(),
        "state": state,
        "url": item.get("url").and_then(Value::as_str).unwrap_or_default(),
        "labels": label_names(item),
        "updatedAt": item.get("updatedAt").and_then(Value::as_str).unwrap_or_default(),
        "author": item.get("author").and_then(|a| a.get("login")).and_then(Value::as_str),
        "assignees": assignees(item),
        "branchName": item.get("headRefName").and_then(Value::as_str),
        "baseRefName": item.get("baseRefName").and_then(Value::as_str),
        "additions": item.get("additions").and_then(Value::as_i64),
        "deletions": item.get("deletions").and_then(Value::as_i64),
        "changedFiles": item.get("changedFiles").and_then(Value::as_i64),
    });
    // Pass through only the values that match the renderer's PRReviewDecision union.
    if let Some(decision) = item.get("reviewDecision").and_then(Value::as_str) {
        if matches!(
            decision,
            "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED"
        ) {
            mapped["reviewDecision"] = json!(decision);
        }
    }
    mapped
}

async fn gh_list(
    kind: WorkItemKind,
    owner: &str,
    repo: &str,
    state: &str,
    search: &str,
    limit: u32,
) -> Result<Vec<Value>, GhError> {
    let (sub, fields) = match kind {
        WorkItemKind::Issue => (
            "issue",
            "id,number,title,state,url,labels,updatedAt,author,assignees",
        ),
        WorkItemKind::Pr => (
            "pr",
            "id,number,title,state,url,labels,updatedAt,author,assignees,headRefName,baseRefName,isDraft,additions,deletions,changedFiles,reviewDecision",
        ),
    };
    let mut args: Vec<String> = vec![
        sub.into(),
        "list".into(),
        "--repo".into(),
        format!("{owner}/{repo}"),
        "--json".into(),
        fields.into(),
        "--limit".into(),
        limit.to_string(),
        "--state".into(),
        gh_state_arg(kind, state).into(),
    ];
    if !search.trim().is_empty() {
        args.push("--search".into());
        args.push(search.trim().into());
    }
    let output = tokio::process::Command::new("gh")
        .args(&args)
        .output()
        .await
        .map_err(|e| GhError {
            kind: "unknown",
            message: format!("Couldn't run gh: {e}"),
        })?;
    if !output.status.success() {
        return Err(classify_gh_error(&String::from_utf8_lossy(&output.stderr)));
    }
    let parsed: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|e| GhError {
        kind: "unknown",
        message: format!("Couldn't parse gh output: {e}"),
    })?;
    Ok(parsed
        .iter()
        .map(|item| match kind {
            WorkItemKind::Issue => map_issue(item),
            WorkItemKind::Pr => map_pr(item),
        })
        .collect())
}

#[tauri::command]
pub async fn gh_repo_slug(repo_path: String, repo_id: Option<String>) -> Option<Value> {
    let _ = repo_id;
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let (owner, repo) = owner_repo_from_remote(&remote)?;
    Some(json!({ "owner": owner, "repo": repo }))
}

// List the repo's branches and current default branch via the authenticated `gh`
// CLI, so the desktop can offer a "change default branch" picker. The default
// branch is a GitHub-side concept (it governs the PR base and the clone HEAD), so
// we read it from the API rather than from local refs — the picker must agree with
// what GitHub will actually change. Returns { ok, branches, default } or, on any
// failure (non-GitHub repo, gh missing/unauthed, network), { ok:false, error } so
// the caller can surface one message. Assumes a local checkout (runs `git -C`
// locally) — same limitation as the sibling gh_* commands; SSH-hosted repos fall
// through to the not-a-GitHub-repo branch.
#[tauri::command]
pub async fn gh_repo_branches(repo_path: String) -> Value {
    let Some((owner, repo)) = resolve_owner_repo(&repo_path).await else {
        return json!({ "ok": false, "error": "Not a GitHub repository (no origin remote)." });
    };
    let slug = format!("{owner}/{repo}");

    // Current default branch.
    let default_out = tokio::process::Command::new("gh")
        .args([
            "repo",
            "view",
            &slug,
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ])
        .output()
        .await;
    let default_out = match default_out {
        Ok(out) => out,
        Err(e) => return json!({ "ok": false, "error": format!("Couldn't run gh: {e}") }),
    };
    if !default_out.status.success() {
        return json!({
            "ok": false,
            "error": classify_gh_error(&String::from_utf8_lossy(&default_out.stderr)).message,
        });
    }
    let default_branch = String::from_utf8_lossy(&default_out.stdout)
        .trim()
        .to_string();

    // Full branch list (paginated; names only).
    let list_out = tokio::process::Command::new("gh")
        .args([
            "api",
            "--paginate",
            &format!("repos/{slug}/branches"),
            "--jq",
            ".[].name",
        ])
        .output()
        .await;
    let list_out = match list_out {
        Ok(out) => out,
        Err(e) => return json!({ "ok": false, "error": format!("Couldn't run gh: {e}") }),
    };
    if !list_out.status.success() {
        return json!({
            "ok": false,
            "error": classify_gh_error(&String::from_utf8_lossy(&list_out.stderr)).message,
        });
    }
    let branches = parse_branch_names(&String::from_utf8_lossy(&list_out.stdout));

    json!({ "ok": true, "branches": branches, "default": default_branch })
}

// Change the repo's GitHub default branch via the authenticated `gh` CLI. This is
// an outward-facing change — it re-targets the base of every new PR and the branch
// fresh clones check out — so the caller gates it behind an explicit confirm. `gh`
// requires admin on the repo; a missing permission surfaces as a 403 that
// classify_gh_error maps to a readable permission message. Returns { ok } or
// { ok:false, error }.
#[tauri::command]
pub async fn gh_set_default_branch(repo_path: String, branch: String) -> Value {
    let Some((owner, repo)) = resolve_owner_repo(&repo_path).await else {
        return json!({ "ok": false, "error": "Not a GitHub repository (no origin remote)." });
    };
    let output = tokio::process::Command::new("gh")
        .args([
            "repo",
            "edit",
            &format!("{owner}/{repo}"),
            "--default-branch",
            &branch,
        ])
        .output()
        .await;
    let output = match output {
        Ok(out) => out,
        Err(e) => return json!({ "ok": false, "error": format!("Couldn't run gh: {e}") }),
    };
    if !output.status.success() {
        return json!({
            "ok": false,
            "error": classify_gh_error(&String::from_utf8_lossy(&output.stderr)).message,
        });
    }
    json!({ "ok": true })
}

#[tauri::command]
pub fn gh_enqueue_pr_refresh() -> bool {
    // No PR-refresh queue is ported; report not enqueued.
    false
}

// The rest of the GitHub namespace needs a token + REST/GraphQL. Mutations and the
// rate-limit query report a not-available failure; boolean acks are false; data
// lookups are null; lists/counts are empty. The richer *BySlug/projects result
// shapes are omitted until the API client lands.
fn not_available() -> Value {
    json!({ "ok": false, "error": "The GitHub API isn't available in this build." })
}

#[tauri::command]
pub fn gh_update_issue() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_merge_pr() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_set_pr_auto_merge() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_request_pr_reviewers() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_pr_state() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_remove_pr_reviewers() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_rerun_pr_checks() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_create_issue() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_rate_limit() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_star_agentum() -> bool {
    false
}

#[tauri::command]
pub fn gh_set_pr_file_viewed() -> bool {
    false
}

#[tauri::command]
pub fn gh_update_pr_title() -> bool {
    false
}

#[tauri::command]
pub fn gh_check_agentum_starred() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item_by_owner_repo() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item_details() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_issue() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_pr_check_details() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_pr_checks() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_count_work_items() -> i64 {
    0
}

// Remaining GitHub surface: REST/GraphQL mutations (issue/PR/comment/project edits)
// report not-available; *BySlug list reads and project-view reads are empty/null;
// fire-and-forget reporting no-ops. All await the API client described above.
#[tauri::command]
pub fn gh_update_pull_request_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_by_slug() -> Value {
    not_available()
}

// Post a comment to a GitHub issue (or PR conversation) via the authenticated `gh`
// CLI, and return the renderer's GitHubCommentResult ({ ok, comment } | { ok:false,
// error }). This both serves the existing issue/PR comment UI (previously stubbed)
// and lets the browser-verification loop (spec 008a) report pass/fail to the linked
// issue. PR conversation comments live under the issues API, so one endpoint covers
// both — `type` is accepted-and-ignored. `prRepo` (cross-repo / fork PRs) overrides
// the origin owner/repo when present.
#[tauri::command]
pub async fn gh_add_issue_comment(
    repo_path: String,
    number: i64,
    body: String,
    pr_repo: Option<Value>,
) -> Value {
    let from_pr_repo = pr_repo.as_ref().and_then(|pr| {
        let owner = pr.get("owner").and_then(Value::as_str)?;
        let repo = pr.get("repo").and_then(Value::as_str)?;
        Some((owner.to_string(), repo.to_string()))
    });
    let owner_repo = match from_pr_repo {
        Some(owner_repo) => Some(owner_repo),
        None => resolve_owner_repo(&repo_path).await,
    };
    let Some((owner, repo)) = owner_repo else {
        return json!({ "ok": false, "error": "Not a GitHub repository (no origin remote)." });
    };
    let output = tokio::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner}/{repo}/issues/{number}/comments"),
            "-f",
            &format!("body={body}"),
        ])
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(e) => return json!({ "ok": false, "error": format!("Couldn't run gh: {e}") }),
    };
    if !output.status.success() {
        return json!({
            "ok": false,
            "error": classify_gh_error(&String::from_utf8_lossy(&output.stderr)).message,
        });
    }
    let created: Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(e) => {
            return json!({ "ok": false, "error": format!("Couldn't parse gh response: {e}") })
        }
    };
    let user = created.get("user");
    json!({
        "ok": true,
        "comment": {
            "id": created.get("id").and_then(Value::as_i64).unwrap_or_default(),
            "author": user.and_then(|u| u.get("login")).and_then(Value::as_str).unwrap_or_default(),
            "authorAvatarUrl": user.and_then(|u| u.get("avatar_url")).and_then(Value::as_str).unwrap_or_default(),
            "body": created.get("body").and_then(Value::as_str).unwrap_or(body.as_str()),
            "createdAt": created.get("created_at").and_then(Value::as_str).unwrap_or_default(),
            "url": created.get("html_url").and_then(Value::as_str).unwrap_or_default(),
        },
    })
}

#[tauri::command]
pub fn gh_add_pr_review_comment_reply() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_pr_review_comment() -> Value {
    not_available()
}

// Fans out to `gh issue list` / `gh pr list` for the repo's origin and returns
// the renderer's ListWorkItemsResult envelope ({ items, sources, errors? }).
// `before` (cursor pagination) isn't expressible via the gh CLI list flags, so
// it's accepted-and-ignored — the first page covers the board's needs.
#[tauri::command]
pub async fn gh_list_work_items(
    repo_path: String,
    repo_id: Option<String>,
    limit: Option<u32>,
    query: Option<String>,
    no_cache: Option<bool>,
    before: Option<String>,
) -> Value {
    let _ = (repo_id, no_cache, before);
    let limit = limit.unwrap_or(30).clamp(1, 100);
    let parsed = parse_query(&query.unwrap_or_default());

    let Some((owner, repo)) = resolve_owner_repo(&repo_path).await else {
        // Folder-mode / non-GitHub repo: empty and error-free so a mixed
        // selection doesn't show a false failure for this entry.
        return json!({
            "items": [],
            "sources": { "issues": null, "prs": null, "upstreamCandidate": null },
        });
    };

    let mut items: Vec<Value> = Vec::new();
    let mut issues_error: Option<Value> = None;
    for kind in &parsed.kinds {
        match gh_list(*kind, &owner, &repo, &parsed.state, &parsed.search, limit).await {
            Ok(mut found) => items.append(&mut found),
            Err(err) => {
                // Only the issues side feeds the per-repo banner (PR-side
                // failures are out of its scope); a "not_found" is benign.
                if *kind == WorkItemKind::Issue && err.kind != "not_found" {
                    issues_error = Some(json!({ "type": err.kind, "message": err.message }));
                } else if *kind == WorkItemKind::Pr {
                    eprintln!("[gh] pr list failed for {owner}/{repo}: {}", err.message);
                }
            }
        }
    }

    let source = json!({ "owner": owner, "repo": repo });
    let mut envelope = json!({
        "items": items,
        "sources": { "issues": source, "prs": source, "upstreamCandidate": null },
    });
    if let Some(error) = issues_error {
        envelope["errors"] = json!({ "issues": error });
    }
    envelope
}

#[tauri::command]
pub fn gh_refresh_pr_now() -> bool {
    false
}

#[tauri::command]
pub fn gh_delete_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_list_issue_types_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_list_labels_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_list_assignable_users_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_update_project_item_field() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_clear_project_item_field() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_type_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_pr_comments() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_resolve_review_thread() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_report_visible_pr_refresh_candidates() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_branch_names_trims_and_drops_blanks() {
        // gh --jq '.[].name' yields one name per line + a trailing newline, and a
        // paginated boundary can leave a blank line between pages.
        let raw = "main\ndevelop\n\n  feature/x  \n";
        assert_eq!(
            parse_branch_names(raw),
            vec![
                "main".to_string(),
                "develop".to_string(),
                "feature/x".to_string(),
            ]
        );
    }

    #[test]
    fn parse_branch_names_empty_or_whitespace_is_empty() {
        assert!(parse_branch_names("").is_empty());
        assert!(parse_branch_names("\n\n  \n").is_empty());
    }

    #[test]
    fn owner_repo_from_remote_handles_https_ssh_and_suffixes() {
        assert_eq!(
            owner_repo_from_remote("https://github.com/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        // scp-like ssh remote
        assert_eq!(
            owner_repo_from_remote("git@github.com:Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        // no .git suffix, trailing slash
        assert_eq!(
            owner_repo_from_remote("https://github.com/Owner/Repo/"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
    }

    #[test]
    fn owner_repo_from_remote_rejects_too_short() {
        assert_eq!(owner_repo_from_remote("justone"), None);
        assert_eq!(owner_repo_from_remote(""), None);
    }

    #[test]
    fn classify_gh_error_maps_403_to_permission_denied() {
        // `gh repo edit` without admin returns a 403 — the set-default-branch path
        // depends on this mapping to tell the user to check their permissions.
        let err = classify_gh_error("HTTP 403: Must have admin access (gh)");
        assert_eq!(err.kind, "permission_denied");
        assert!(!err.message.is_empty());
    }

    #[test]
    fn classify_gh_error_maps_unresolved_repo_to_not_found() {
        let err = classify_gh_error("Could not resolve to a Repository with the name 'x/y'.");
        assert_eq!(err.kind, "not_found");
    }
}
