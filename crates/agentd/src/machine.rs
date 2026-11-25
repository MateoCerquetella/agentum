//! What the hub learns about this machine: agents, projects, load and memory.
use crate::config::Config;
use crate::util::{home, parse_version, run, which};
use agentum_proto::AgentKind;
use agentum_proto::machine::{AgentInfo, Project, Stats};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub const MAX_PROJECTS: usize = 400;
pub const MAX_DEPTH: usize = 4;

pub fn binary_name(a: AgentKind) -> &'static str {
    match a {
        AgentKind::ClaudeCode => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Opencode => "opencode",
    }
}

/// The agent's CLI: the configured one, or the first on PATH.
pub fn agent_bin(cfg: &Config, a: AgentKind) -> Option<PathBuf> {
    match cfg.agent_bin(a) {
        Some(p) => {
            let p = crate::util::expand(p);
            p.is_file().then_some(p)
        }
        None => which(binary_name(a)),
    }
}

pub async fn agents(cfg: &Config) -> Vec<AgentInfo> {
    let futs = AgentKind::ALL.into_iter().map(|a| {
        let bin = agent_bin(cfg, a);
        async move {
            let Some(bin) = bin else { return AgentInfo { agent: a, installed: false, version: None, path: None } };
            let mut cmd = Command::new(&bin);
            cmd.arg("--version");
            let version = match run(&mut cmd, Duration::from_secs(15)).await {
                Ok(o) => parse_version(&o.stdout).or_else(|| parse_version(&o.stderr)),
                Err(e) => {
                    tracing::info!("{} --version: {e}", bin.display());
                    None
                }
            };
            AgentInfo { agent: a, installed: true, version, path: Some(bin.to_string_lossy().into_owned()) }
        }
    });
    futures::future::join_all(futs).await
}

/// The current branch of a repo, from its HEAD file (no git process).
pub fn branch(repo: &Path) -> Option<String> {
    let dotgit = repo.join(".git");
    let gitdir = if dotgit.is_dir() {
        dotgit
    } else {
        let t = std::fs::read_to_string(&dotgit).ok()?;
        let p = PathBuf::from(t.strip_prefix("gitdir:")?.trim());
        if p.is_absolute() { p } else { repo.join(p) }
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(b) => Some(b.to_string()),
        None => Some(head.chars().take(10).collect()),
    }
}

pub fn is_repo(p: &Path) -> bool {
    p.join(".git").exists()
}

fn project(p: &Path) -> Project {
    Project {
        path: p.to_string_lossy().into_owned(),
        name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned()),
        branch: branch(p),
    }
}

const SKIP: [&str; 8] = ["node_modules", "target", "vendor", "dist", "build", "__pycache__", "venv", "Library"];

/// Git repos under the roots (depth ≤ 4, at most 400), then the explicit ones. Blocking.
pub fn scan_projects(roots: &[PathBuf], extra: &[PathBuf], skip: &[PathBuf]) -> Vec<Project> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for p in extra {
        if let Ok(c) = p.canonicalize()
            && seen.insert(c.clone())
        {
            out.push(project(&c));
        }
    }
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().rev().map(|r| (r.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= MAX_PROJECTS {
            break;
        }
        if skip.iter().any(|s| dir.starts_with(s)) {
            continue;
        }
        if is_repo(&dir) {
            if let Ok(c) = dir.canonicalize()
                && seen.insert(c.clone())
            {
                out.push(project(&c));
            }
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                !n.starts_with('.') && !SKIP.contains(&n.as_ref())
            })
            .map(|e| e.path())
            .collect();
        subs.sort();
        for s in subs.into_iter().rev() {
            stack.push((s, depth + 1));
        }
    }
    out
}

fn meminfo() -> (u64, u64) {
    let t = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| t.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) / 1024;
    (get("MemTotal:"), get("MemAvailable:"))
}

pub fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

pub fn stats() -> Stats {
    let (total, avail) = meminfo();
    Stats {
        cpus: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
        load1: std::fs::read_to_string("/proc/loadavg").ok().and_then(|t| t.split_whitespace().next().and_then(|v| v.parse().ok())).unwrap_or(0.0),
        mem_total_mb: total,
        mem_avail_mb: avail,
        rss_mb: rss_kb() / 1024,
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// "Debian GNU/Linux 12 (bookworm)", else "linux".
pub fn os() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

pub fn home_string() -> String {
    home().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_repos_and_reads_branches() {
        let root = std::env::temp_dir().join(format!("agentd-scan-{}", crate::util::random_hex(4)));
        let a = root.join("org/a");
        let deep = root.join("1/2/3/4/5/deep");
        for r in [&a, &deep] {
            std::fs::create_dir_all(r.join(".git")).unwrap();
            std::fs::write(r.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        }
        std::fs::create_dir_all(root.join("node_modules/b/.git")).unwrap();
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", a.join(".git").display())).unwrap();
        let found = scan_projects(&[root.clone()], &[], &[]);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "wt"]);
        assert_eq!(found[0].branch.as_deref(), Some("feature/x"));
        assert_eq!(found[1].branch.as_deref(), Some("feature/x"));
        // Skipped directories stay out.
        let found = scan_projects(&[root.clone()], &[], &[root.join("wt")]);
        assert_eq!(found.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stats_from_proc() {
        let s = stats();
        assert!(s.cpus >= 1);
        assert!(s.mem_total_mb > 0);
        assert!(s.rss_mb < 1024);
    }
}
