//! Git for the hub (shelling out to `git`, and `gh` for pull requests).
use crate::util::{Out, run};
use agentum_proto::machine::GitReq;
use anyhow::bail;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

async fn git(cwd: &Path, args: &[&str], timeout: u64) -> anyhow::Result<Out> {
    let mut c = Command::new("git");
    c.arg("-C").arg(cwd).args(args).env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C").env("GIT_OPTIONAL_LOCKS", "0");
    run(&mut c, Duration::from_secs(timeout)).await
}

/// stdout of a git command that must succeed.
async fn git_ok(cwd: &Path, args: &[&str], timeout: u64) -> anyhow::Result<String> {
    let o = git(cwd, args, timeout).await?;
    if !o.ok {
        bail!("git {}: {}", args.first().unwrap_or(&""), first_lines(&o.stderr, &o.stdout));
    }
    Ok(o.stdout)
}

fn first_lines(a: &str, b: &str) -> String {
    let t = if a.trim().is_empty() { b } else { a };
    t.trim().lines().take(8).collect::<Vec<_>>().join("\n")
}

/// Parse `git status --porcelain=v2 -z --branch`.
pub fn parse_status(out: &str) -> Value {
    let mut branch = None;
    let mut upstream = None;
    let (mut ahead, mut behind) = (0i64, 0i64);
    let mut oid = None;
    let mut files = Vec::new();
    let mut it = out.split('\0').peekable();
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.head" if v != "(detached)" => branch = Some(v.to_string()),
                "branch.oid" if v != "(initial)" => oid = Some(v.to_string()),
                "branch.upstream" => upstream = Some(v.to_string()),
                "branch.ab" => {
                    for p in v.split_whitespace() {
                        if let Some(a) = p.strip_prefix('+') {
                            ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = p.strip_prefix('-') {
                            behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.chars().next().unwrap_or(' ');
        match kind {
            '1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = rec.splitn(9, ' ').collect();
                if parts.len() == 9 {
                    files.push(json!({"path": parts[8], "status": parts[1]}));
                }
            }
            '2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let parts: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = it.next().unwrap_or("");
                if parts.len() == 10 {
                    files.push(json!({"path": parts[9], "status": parts[1], "orig": orig}));
                }
            }
            'u' => {
                let parts: Vec<&str> = rec.splitn(11, ' ').collect();
                if parts.len() == 11 {
                    files.push(json!({"path": parts[10], "status": parts[1], "conflict": true}));
                }
            }
            '?' => files.push(json!({"path": &rec[2..], "status": "??"})),
            '!' => files.push(json!({"path": &rec[2..], "status": "!!"})),
            _ => {}
        }
    }
    json!({"branch": branch, "head": oid, "upstream": upstream, "ahead": ahead, "behind": behind, "files": files})
}

/// Parse `git log --format=%H%x1f%s%x1f%an%x1f%at%x1e`.
pub fn parse_log(out: &str) -> Vec<Value> {
    out.split('\x1e')
        .filter_map(|r| {
            let r = r.trim_start_matches('\n');
            let f: Vec<&str> = r.split('\x1f').collect();
            (f.len() >= 4).then(|| json!({"sha": f[0], "subject": f[1], "author": f[2], "ts": f[3].trim().parse::<i64>().unwrap_or(0) * 1000}))
        })
        .collect()
}

/// The branch new work is compared with: origin's default, else main/master.
async fn base_ref(cwd: &Path) -> Option<String> {
    if let Ok(o) = git_ok(cwd, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], 10).await {
        let r = o.trim().to_string();
        if !r.is_empty() {
            return Some(r);
        }
    }
    for b in ["origin/main", "origin/master", "main", "master"] {
        if git(cwd, &["rev-parse", "--verify", "--quiet", b], 10).await.map(|o| o.ok).unwrap_or(false) {
            return Some(b.to_string());
        }
    }
    None
}

/// Diffs of untracked files (as additions), for "uncommitted".
async fn untracked_diff(cwd: &Path, path: Option<&str>) -> String {
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let Ok(list) = git_ok(cwd, &args, 30).await else { return String::new() };
    let mut out = String::new();
    for f in list.split('\0').filter(|f| !f.is_empty()).take(100) {
        if std::fs::metadata(cwd.join(f)).map(|m| m.len() > 1024 * 1024).unwrap_or(true) {
            continue;
        }
        if let Ok(o) = git(cwd, &["diff", "--no-index", "--", "/dev/null", f], 30).await {
            out.push_str(&o.stdout);
        }
    }
    out
}

pub async fn handle(req: GitReq, worktrees: &Path) -> anyhow::Result<Value> {
    match req {
        GitReq::Status { cwd } => {
            let cwd = PathBuf::from(cwd);
            let out = git_ok(&cwd, &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"], 60).await?;
            Ok(parse_status(&out))
        }
        GitReq::Diff { cwd, target, path } => {
            let cwd = PathBuf::from(cwd);
            let target = target.unwrap_or_else(|| "uncommitted".into());
            let has_head = git(&cwd, &["rev-parse", "--verify", "--quiet", "HEAD"], 10).await.map(|o| o.ok).unwrap_or(false);
            let mut args: Vec<String> = vec!["diff".into(), "--no-color".into(), "--no-ext-diff".into()];
            match target.as_str() {
                "uncommitted" => {
                    if has_head {
                        args.push("HEAD".into());
                    }
                }
                "staged" => args.push("--cached".into()),
                "branch" => {
                    let base = base_ref(&cwd).await.ok_or_else(|| anyhow::anyhow!("no base branch to compare with"))?;
                    let mb = git_ok(&cwd, &["merge-base", "HEAD", &base], 30).await?;
                    args.push(mb.trim().to_string());
                    args.push("HEAD".into());
                }
                sha => {
                    if sha.starts_with('-') {
                        bail!("bad target {sha}");
                    }
                    // `show` works for a root commit too.
                    args = vec!["show".into(), "--format=".into(), "--no-color".into(), "--patch".into(), sha.to_string()];
                }
            }
            if let Some(p) = &path {
                args.push("--".into());
                args.push(p.clone());
            }
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let mut diff = git_ok(&cwd, &a, 120).await?;
            if target == "uncommitted" {
                diff.push_str(&untracked_diff(&cwd, path.as_deref()).await);
            }
            Ok(json!({"target": target, "diff": diff}))
        }
        GitReq::Commit { cwd, message, all } => {
            let cwd = PathBuf::from(cwd);
            if message.trim().is_empty() {
                bail!("empty commit message");
            }
            // "All" means everything, new files too (as people expect), not only `-a`'s tracked ones.
            if all {
                git_ok(&cwd, &["add", "-A"], 60).await?;
            }
            git_ok(&cwd, &["commit", "-m", &message], 120).await?;
            let sha = git_ok(&cwd, &["rev-parse", "HEAD"], 10).await?;
            Ok(json!({"sha": sha.trim()}))
        }
        GitReq::Push { cwd } => {
            let cwd = PathBuf::from(cwd);
            let has_upstream = git(&cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], 10).await.map(|o| o.ok).unwrap_or(false);
            let o = if has_upstream { git(&cwd, &["push"], 300).await? } else { git(&cwd, &["push", "-u", "origin", "HEAD"], 300).await? };
            if !o.ok {
                bail!("git push: {}", first_lines(&o.stderr, &o.stdout));
            }
            Ok(json!({"output": format!("{}{}", o.stdout, o.stderr).trim()}))
        }
        GitReq::Log { cwd, limit, path } => {
            let cwd = PathBuf::from(cwd);
            let n = format!("-n{}", limit.unwrap_or(50).min(1000));
            let mut args = vec!["log", &n, "--format=%H%x1f%s%x1f%an%x1f%at%x1e"];
            if let Some(p) = &path {
                args.push("--");
                args.push(p);
            }
            let o = git(&cwd, &args, 60).await?;
            if !o.ok {
                // A repo without commits has no log.
                if o.stderr.contains("does not have any commits") {
                    return Ok(json!([]));
                }
                bail!("git log: {}", first_lines(&o.stderr, &o.stdout));
            }
            Ok(Value::Array(parse_log(&o.stdout)))
        }
        GitReq::Show { cwd, sha } => {
            let cwd = PathBuf::from(cwd);
            if sha.starts_with('-') {
                bail!("bad sha {sha}");
            }
            let head = git_ok(&cwd, &["show", "-s", "--format=%H%x1f%s%x1f%an%x1f%at%x1f%b", &sha], 30).await?;
            let f: Vec<&str> = head.splitn(5, '\x1f').collect();
            let diff = git_ok(&cwd, &["show", "--format=", "--no-color", "--patch", &sha], 120).await?;
            Ok(json!({
                "sha": f.first().copied().unwrap_or(""),
                "subject": f.get(1).copied().unwrap_or(""),
                "author": f.get(2).copied().unwrap_or(""),
                "ts": f.get(3).and_then(|t| t.trim().parse::<i64>().ok()).unwrap_or(0) * 1000,
                "body": f.get(4).map(|b| b.trim()).unwrap_or(""),
                "diff": diff,
            }))
        }
        GitReq::Branches { cwd } => {
            let cwd = PathBuf::from(cwd);
            let out = git_ok(&cwd, &["for-each-ref", "--format=%(refname:short)%1f%(objectname)%1f%(HEAD)%1f%(upstream:short)%1f%(committerdate:unix)", "refs/heads"], 30).await?;
            let list: Vec<Value> = out
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split('\x1f').collect();
                    (f.len() >= 5).then(|| {
                        json!({"name": f[0], "sha": f[1], "current": f[2] == "*", "upstream": (!f[3].is_empty()).then_some(f[3]), "ts": f[4].parse::<i64>().unwrap_or(0) * 1000})
                    })
                })
                .collect();
            Ok(Value::Array(list))
        }
        GitReq::Pr { cwd } => {
            let mut c = Command::new("gh");
            c.args(["pr", "view", "--json", "number,title,state,url,isDraft,mergeable,reviewDecision,statusCheckRollup,headRefName"]).current_dir(&cwd).env("GH_PROMPT_DISABLED", "1");
            let o = run(&mut c, Duration::from_secs(60)).await?;
            if !o.ok {
                let e = o.stderr.to_lowercase();
                if e.contains("no pull requests found") || e.contains("no default remote") || e.contains("not a git repository") || e.contains("none of the git remotes") || e.contains("no git remotes") {
                    return Ok(Value::Null);
                }
                bail!("gh pr view: {}", first_lines(&o.stderr, &o.stdout));
            }
            Ok(serde_json::from_str(&o.stdout)?)
        }
        GitReq::PrMerge { cwd, method } => {
            let flag = match method.as_deref().unwrap_or("squash") {
                "merge" => "--merge",
                "rebase" => "--rebase",
                _ => "--squash",
            };
            let mut c = Command::new("gh");
            c.args(["pr", "merge", flag]).current_dir(&cwd).env("GH_PROMPT_DISABLED", "1");
            let o = run(&mut c, Duration::from_secs(180)).await?;
            if !o.ok {
                bail!("gh pr merge: {}", first_lines(&o.stderr, &o.stdout));
            }
            Ok(json!({"output": format!("{}{}", o.stdout, o.stderr).trim()}))
        }
        GitReq::WorktreeAdd { repo, branch, base } => {
            let repo = PathBuf::from(repo);
            if branch.trim().is_empty() || branch.starts_with('-') || branch.contains("..") {
                bail!("bad branch name {branch:?}");
            }
            let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
            let safe: String = branch.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
            std::fs::create_dir_all(worktrees)?;
            let path = worktrees.join(format!("{name}-{safe}"));
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            let p = path.to_string_lossy().into_owned();
            let exists = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], 10).await.map(|o| o.ok).unwrap_or(false);
            if exists {
                git_ok(&repo, &["worktree", "add", &p, &branch], 300).await?;
            } else {
                let base = base.unwrap_or_else(|| "HEAD".into());
                if base.starts_with('-') {
                    bail!("bad base {base}");
                }
                git_ok(&repo, &["worktree", "add", "-b", &branch, &p, &base], 300).await?;
            }
            Ok(json!({"path": p, "branch": branch}))
        }
        GitReq::WorktreeRemove { path } => {
            let path = PathBuf::from(path);
            // Run it from the main repo (a worktree can't remove itself).
            let common = git_ok(&path, &["rev-parse", "--path-format=absolute", "--git-common-dir"], 10).await?;
            let main = PathBuf::from(common.trim()).parent().map(Path::to_path_buf).ok_or_else(|| anyhow::anyhow!("no main repo"))?;
            git_ok(&main, &["worktree", "remove", &path.to_string_lossy()], 120).await?;
            let _ = git(&main, &["worktree", "prune"], 30).await;
            Ok(json!({"removed": path}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_v2() {
        let out = "# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\01 .M N... 100644 100644 100644 aaa bbb src/a b.rs\02 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0? notes.txt\0";
        let v = parse_status(out);
        assert_eq!(v["branch"], "main");
        assert_eq!(v["upstream"], "origin/main");
        assert_eq!((v["ahead"].as_i64(), v["behind"].as_i64()), (Some(2), Some(1)));
        assert_eq!(v["files"][0], json!({"path": "src/a b.rs", "status": ".M"}));
        assert_eq!(v["files"][1], json!({"path": "new.rs", "status": "R.", "orig": "old.rs"}));
        assert_eq!(v["files"][2], json!({"path": "notes.txt", "status": "??"}));
    }

    #[test]
    fn log_records() {
        let l = parse_log("abc\x1fFix it\x1fMe\x1f1700000000\x1e\ndef\x1fMore\x1fYou\x1f1700000001\x1e\n");
        assert_eq!(l.len(), 2);
        assert_eq!(l[1]["ts"], 1700000001000i64);
    }
}
