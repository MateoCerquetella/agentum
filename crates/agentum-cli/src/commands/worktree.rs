//! `agentum worktree` — list the worktrees the control plane knows about and
//! resolve which one you're standing in. Backed by the server's existing
//! `/api/worktrees` route (no new endpoint); reaches the desktop's embedded
//! server when run inside a pane via `$AGENTUM_API_URL`.

use anyhow::Result;
use serde_json::Value;

use crate::http::ApiClient;

/// Worktree filesystem path. Persisted ids are `repoId::path`; fall back to a
/// flattened `path` field if present. `None` when neither is decodable.
fn worktree_path(wt: &Value) -> Option<String> {
    if let Some(id) = wt.get("id").and_then(Value::as_str) {
        if let Some((_, p)) = id.split_once("::") {
            return Some(p.to_string());
        }
    }
    wt.get("path").and_then(Value::as_str).map(str::to_string)
}

fn field<'a>(wt: &'a Value, key: &str) -> &'a str {
    wt.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Human label for a worktree: its `displayName`, or the path basename when the
/// registry left the name blank (common for remote/SSH worktrees), or
/// `(unnamed)` as a last resort. Keeps the table/`current` output legible
/// instead of showing an empty column.
fn display_label(wt: &Value) -> String {
    let name = field(wt, "displayName");
    if !name.is_empty() {
        return name.to_string();
    }
    if let Some(path) = worktree_path(wt) {
        if let Some(base) = path.rsplit('/').find(|s| !s.is_empty()) {
            return base.to_string();
        }
    }
    "(unnamed)".to_string()
}

/// The worktree whose path contains `cwd` — the deepest (longest) match wins so
/// a nested worktree beats its parent repo. Pure, for unit testing.
pub fn find_current_worktree<'a>(worktrees: &'a [Value], cwd: &str) -> Option<&'a Value> {
    worktrees
        .iter()
        .filter_map(|wt| worktree_path(wt).map(|p| (wt, p)))
        .filter(|(_, p)| cwd == p || cwd.starts_with(&format!("{p}/")))
        .max_by_key(|(_, p)| p.len())
        .map(|(wt, _)| wt)
}

async fn fetch(client: &ApiClient) -> Result<Vec<Value>> {
    Ok(client
        .get_json("/api/worktrees")
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn print_table(worktrees: &[Value]) {
    if worktrees.is_empty() {
        println!("(no worktrees)");
        return;
    }
    let name_w = worktrees
        .iter()
        .map(|w| display_label(w).len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!("{:<nw$}  branch", "NAME", nw = name_w);
    for w in worktrees {
        let branch = field(w, "branch");
        println!(
            "{:<nw$}  {}",
            display_label(w),
            if branch.is_empty() { "-" } else { branch },
            nw = name_w
        );
    }
}

pub async fn list(json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let worktrees = fetch(&client).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&worktrees)?);
    } else {
        print_table(&worktrees);
    }
    Ok(())
}

pub async fn current(json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let worktrees = fetch(&client).await?;
    // Prefer the explicit pane env (the worktree the desktop launched this pane
    // for); fall back to the process cwd.
    let cwd = std::env::var("AGENTUM_WORKTREE_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    match find_current_worktree(&worktrees, &cwd) {
        Some(wt) if json => println!("{}", serde_json::to_string_pretty(wt)?),
        Some(wt) => {
            let branch = field(wt, "branch");
            println!(
                "{}{}",
                display_label(wt),
                if branch.is_empty() {
                    String::new()
                } else {
                    format!("  ({branch})")
                }
            );
        }
        None if json => println!("null"),
        None => {
            println!("(not inside a known worktree: {cwd})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_matches_deepest_worktree_by_path() {
        let wts = vec![
            json!({"id": "repo::/home/u/proj", "displayName": "proj"}),
            json!({"id": "repo::/home/u/proj/.agentum/worktrees/feat", "displayName": "feat"}),
        ];
        // Standing inside the nested worktree → the nested one wins, not the parent.
        let got = find_current_worktree(&wts, "/home/u/proj/.agentum/worktrees/feat/src");
        assert_eq!(got.unwrap()["displayName"], "feat");
        // Standing in the parent repo → the parent.
        let got = find_current_worktree(&wts, "/home/u/proj");
        assert_eq!(got.unwrap()["displayName"], "proj");
    }

    #[test]
    fn current_is_none_outside_any_worktree() {
        let wts = vec![json!({"id": "repo::/home/u/proj", "displayName": "proj"})];
        assert!(find_current_worktree(&wts, "/tmp/elsewhere").is_none());
    }

    #[test]
    fn path_falls_back_to_flat_field() {
        let wt = json!({"path": "/x/y", "displayName": "y"});
        assert_eq!(worktree_path(&wt).as_deref(), Some("/x/y"));
    }

    #[test]
    fn display_label_falls_back_to_basename_then_unnamed() {
        assert_eq!(display_label(&json!({"displayName": "feat"})), "feat");
        // Empty name (common for remote worktrees) → path basename.
        assert_eq!(
            display_label(&json!({"id": "repo::/a/b/my-wt", "displayName": ""})),
            "my-wt"
        );
        // No name and no path → a legible placeholder, never blank.
        assert_eq!(display_label(&json!({"displayName": ""})), "(unnamed)");
    }
}
