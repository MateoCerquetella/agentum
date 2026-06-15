use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

// Workspace port scanning lists the local dev servers a user has running and
// attributes each listener to the worktree it was launched from, so the sidebar
// / status bar / Ports panel can show "this worktree has :3000 live" with
// open/copy/stop actions. Attribution is by process working directory: a listener
// whose owning PID's cwd is inside a registered worktree path is `kind:
// "workspace"`; every other loopback dev listener is `kind: "external"`.
//
// macOS + Linux shell out to `lsof` (already the codebase's subprocess style —
// see worktrees.rs's git calls). Windows has no `lsof`, so it degrades to an
// "unavailable" scan rather than a hard error, exactly like the prior stub.

/// One worktree from the registry, reduced to the fields attribution needs.
#[derive(Debug, Clone)]
struct WorktreeEntry {
    /// `repoId::/abs/path` — also the `owner.worktreeId` the renderer matches on.
    id: String,
    repo_id: String,
    display_name: String,
    /// The `/abs/path` half of the id; the directory a listener's cwd must be
    /// inside to belong to this worktree.
    path: String,
}

/// A listening socket parsed out of `lsof`, before worktree attribution.
#[derive(Debug, Clone)]
struct Listener {
    pid: u32,
    process_name: String,
    /// Address `lsof` reported the socket bound to (may be `*` / `0.0.0.0` / `::`).
    bind_host: String,
    port: u16,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

/// `~/.agentum/worktrees.json` — the same registry the embedded server reads
/// (`agentum-server::routes::worktrees`). Desktop and server share `$HOME`, so
/// reading it directly here avoids a self-loopback HTTP hop for a pure file read.
fn worktrees_registry_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".agentum").join("worktrees.json"))
}

#[derive(Debug, Deserialize)]
struct RegistryRow {
    id: String,
    #[serde(rename = "repoId")]
    repo_id: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
}

/// Load worktrees from the registry. The path lives in the id (`repoId::path`),
/// matching the server's own `enrich_worktree`. Tolerates a missing/corrupt
/// registry by returning an empty list (scan then reports only `external` ports).
fn load_worktrees() -> Vec<WorktreeEntry> {
    let Some(path) = worktrees_registry_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let rows: Vec<RegistryRow> = serde_json::from_str(&raw).unwrap_or_default();
    rows.into_iter()
        .filter_map(|row| {
            let path = row.id.split_once("::").map(|(_, p)| p.to_string())?;
            (!path.is_empty()).then(|| WorktreeEntry {
                id: row.id,
                repo_id: row.repo_id,
                display_name: row.display_name,
                path,
            })
        })
        .collect()
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -F pcn` field output into listeners.
///
/// `-F pcn` emits one field per line, prefixed by a type char: `p<pid>`,
/// `c<command>`, `n<addr:port>`. Records are pid-grouped — a `p`/`c` pair is
/// followed by one `n` line per listening fd. `-nP` keeps host+port numeric so we
/// don't have to undo name/service resolution. Bracketed IPv6 (`[::1]:3000`) and
/// wildcard binds (`*:3000`) both parse; the port is the segment after the last
/// `:`. Lines we can't parse are skipped rather than failing the whole scan.
fn parse_lsof_listeners(output: &str) -> Vec<Listener> {
    let mut listeners = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command = String::new();
    for line in output.lines() {
        let Some((tag, rest)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => {
                pid = rest.parse().ok();
                command.clear();
            }
            "c" => command = rest.to_string(),
            "n" => {
                let Some(pid) = pid else { continue };
                let Some((bind_host, port)) = split_host_port(rest) else {
                    continue;
                };
                listeners.push(Listener {
                    pid,
                    process_name: command.clone(),
                    bind_host,
                    port,
                });
            }
            _ => {}
        }
    }
    listeners
}

/// Split an `lsof` address (`host:port`) into its host and numeric port. The port
/// is the final colon-delimited segment; everything before is the host, with the
/// IPv6 brackets stripped (`[::1]:3000` -> `::1`). Returns `None` when there's no
/// numeric port (e.g. a half-open `*:*` entry).
fn split_host_port(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

/// Parse `lsof -a -p <pids> -d cwd -F n` into a pid -> cwd map. Same field format
/// as the listener parse: `p<pid>` then `n<cwd>`. We only keep the cwd line.
fn parse_lsof_cwds(output: &str) -> BTreeMap<u32, String> {
    let mut cwds = BTreeMap::new();
    let mut pid: Option<u32> = None;
    for line in output.lines() {
        let Some((tag, rest)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => pid = rest.parse().ok(),
            "n" => {
                if let Some(pid) = pid {
                    cwds.insert(pid, rest.to_string());
                }
            }
            _ => {}
        }
    }
    cwds
}

/// Normalize a wildcard bind to a loopback host the renderer can actually open.
/// `*` / `0.0.0.0` -> `127.0.0.1`; `::` -> `::1`; anything else passes through.
fn connect_host_for(bind_host: &str) -> &str {
    match bind_host {
        "*" | "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    }
}

/// Attribute a listener to a worktree by cwd containment. Picks the longest
/// matching worktree path so a nested worktree wins over its parent repo. Returns
/// the owning worktree, if any.
fn owner_for_cwd<'a>(cwd: &str, worktrees: &'a [WorktreeEntry]) -> Option<&'a WorktreeEntry> {
    worktrees
        .iter()
        .filter(|wt| path_contains(&wt.path, cwd))
        .max_by_key(|wt| wt.path.len())
}

/// True when `cwd` is `base` or a descendant of it. Compares component-wise so
/// `/a/foobar` doesn't count as inside `/a/foo`.
fn path_contains(base: &str, cwd: &str) -> bool {
    let base = Path::new(base);
    let cwd = Path::new(cwd);
    cwd == base || cwd.starts_with(base)
}

/// `https` for the conventional TLS dev ports, else `http`. The renderer treats
/// `unknown` as http anyway; we only special-case the ports a dev server almost
/// always serves TLS on so "Open in Browser" picks the right scheme.
fn protocol_for(port: u16) -> &'static str {
    if matches!(port, 443 | 8443) {
        "https"
    } else {
        "http"
    }
}

fn is_wildcard(host: &str) -> bool {
    matches!(host, "*" | "0.0.0.0" | "::")
}

/// Build the `WorkspacePort[]` JSON from parsed listeners + cwds + registry.
///
/// Deduplicates on `(pid, port)`: a dev server typically binds both IPv4 and IPv6
/// (two `lsof` rows) but is one logical port to the user. The first row wins,
/// except a non-wildcard bind is preferred so the displayed address is concrete.
fn build_ports(
    listeners: &[Listener],
    cwds: &BTreeMap<u32, String>,
    worktrees: &[WorktreeEntry],
) -> Vec<Value> {
    let mut by_key: BTreeMap<(u32, u16), &Listener> = BTreeMap::new();
    for listener in listeners {
        by_key
            .entry((listener.pid, listener.port))
            .and_modify(|existing| {
                // Prefer a concrete bind over a wildcard for the shown address.
                if is_wildcard(&existing.bind_host) && !is_wildcard(&listener.bind_host) {
                    *existing = listener;
                }
            })
            .or_insert(listener);
    }

    by_key
        .values()
        .map(|listener| {
            let connect_host = connect_host_for(&listener.bind_host);
            let mut port = json!({
                "id": format!("{}-{}", listener.pid, listener.port),
                "bindHost": listener.bind_host,
                "connectHost": connect_host,
                "port": listener.port,
                "pid": listener.pid,
                "processName": listener.process_name,
                "protocol": protocol_for(listener.port),
            });
            let owner = cwds
                .get(&listener.pid)
                .and_then(|cwd| owner_for_cwd(cwd, worktrees));
            if let Some(owner) = owner {
                port["kind"] = json!("workspace");
                port["owner"] = json!({
                    "worktreeId": owner.id,
                    "repoId": owner.repo_id,
                    "displayName": owner.display_name,
                    "path": owner.path,
                    "confidence": "cwd",
                });
            } else {
                port["kind"] = json!("external");
            }
            port
        })
        .collect()
}

/// Run `lsof` and assemble the scan, or `None` on macOS/Linux when `lsof` is
/// missing/failed (Windows callers never reach here). When `repo_id` is set, only
/// ports owned by that repo's worktrees stay `workspace`; this matches the
/// per-repo scans the Ports panel requests.
fn scan_with_lsof(repo_id: Option<&str>) -> Option<Vec<Value>> {
    let listing = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .output()
        .ok()?;
    // lsof exits non-zero when *some* handles are unreadable but still prints the
    // readable ones, so we parse stdout regardless of exit status.
    let listeners = parse_lsof_listeners(&String::from_utf8_lossy(&listing.stdout));
    if listeners.is_empty() {
        return Some(Vec::new());
    }

    let mut worktrees = load_worktrees();
    if let Some(repo_id) = repo_id {
        worktrees.retain(|wt| wt.repo_id == repo_id);
    }

    // One cwd lookup for the exact set of listening pids.
    let pids: BTreeSet<u32> = listeners.iter().map(|l| l.pid).collect();
    let pid_csv = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let cwds = Command::new("lsof")
        .args(["-a", "-p", &pid_csv, "-d", "cwd", "-F", "n"])
        .output()
        .ok()
        .map(|out| parse_lsof_cwds(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default();

    Some(build_ports(&listeners, &cwds, &worktrees))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePortScanRequest {
    #[serde(default)]
    repo_id: Option<String>,
}

#[tauri::command]
pub async fn workspace_ports_scan(request: Option<WorkspacePortScanRequest>) -> Value {
    let repo_id = request.and_then(|r| r.repo_id);
    // lsof is a blocking subprocess; keep it off the async runtime threads.
    let ports = tokio::task::spawn_blocking(move || {
        if cfg!(target_os = "windows") {
            None
        } else {
            scan_with_lsof(repo_id.as_deref())
        }
    })
    .await
    .ok()
    .flatten();

    match ports {
        Some(ports) => json!({
            "platform": platform(),
            "scannedAt": now_millis(),
            "ports": ports,
        }),
        None => json!({
            "platform": platform(),
            "scannedAt": now_millis(),
            "ports": [],
            "unavailableReason": "Port scanning needs `lsof`, which isn't available on this system.",
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePortKillRequest {
    pid: u32,
    // port/repoId arrive from the renderer's kill request but the kill itself
    // only needs the pid; kept so the wire shape stays self-documenting.
    #[allow(dead_code)]
    port: u16,
    #[allow(dead_code)]
    repo_id: Option<String>,
}

#[tauri::command]
pub async fn workspace_ports_kill(request: WorkspacePortKillRequest) -> Value {
    let pid = request.pid;
    let result = tokio::task::spawn_blocking(move || kill_pid(pid))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match result {
        Ok(()) => json!({ "ok": true }),
        Err(reason) => json!({ "ok": false, "reason": reason }),
    }
}

/// Terminate a process by pid. Uses `kill` on Unix and `taskkill` on Windows,
/// returning a human-readable reason on failure for the renderer's toast.
fn kill_pid(pid: u32) -> Result<(), String> {
    let output = if cfg!(target_os = "windows") {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
    } else {
        Command::new("kill").arg(pid.to_string()).output()
    };
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            Err(if stderr.is_empty() {
                format!("Failed to stop process {pid}")
            } else {
                stderr.to_string()
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(id: &str, repo: &str, name: &str) -> WorktreeEntry {
        let path = id.split_once("::").map(|(_, p)| p.to_string()).unwrap();
        WorktreeEntry {
            id: id.to_string(),
            repo_id: repo.to_string(),
            display_name: name.to_string(),
            path,
        }
    }

    #[test]
    fn parses_lsof_listener_fields() {
        // Two pids, one with IPv4+IPv6 rows, one wildcard bind.
        let out = "p4242\ncnode\nn127.0.0.1:3000\nn[::1]:3000\np99\ncvite\nn*:5173\n";
        let listeners = parse_lsof_listeners(out);
        assert_eq!(listeners.len(), 3);
        assert_eq!(listeners[0].pid, 4242);
        assert_eq!(listeners[0].process_name, "node");
        assert_eq!(listeners[0].bind_host, "127.0.0.1");
        assert_eq!(listeners[0].port, 3000);
        assert_eq!(listeners[1].bind_host, "::1"); // brackets stripped
        assert_eq!(listeners[2].bind_host, "*");
        assert_eq!(listeners[2].port, 5173);
    }

    #[test]
    fn split_host_port_handles_ipv6_and_wildcard() {
        assert_eq!(
            split_host_port("127.0.0.1:8080"),
            Some(("127.0.0.1".into(), 8080))
        );
        assert_eq!(split_host_port("[::1]:8080"), Some(("::1".into(), 8080)));
        assert_eq!(split_host_port("*:80"), Some(("*".into(), 80)));
        assert_eq!(split_host_port("*:*"), None);
    }

    #[test]
    fn attributes_listener_to_worktree_by_cwd() {
        let worktrees = vec![worktree("repoA::/work/featureA", "repoA", "Feature A")];
        let listeners = parse_lsof_listeners("p10\ncnode\nn127.0.0.1:3000\n");
        let mut cwds = BTreeMap::new();
        cwds.insert(10u32, "/work/featureA/packages/web".to_string());

        let ports = build_ports(&listeners, &cwds, &worktrees);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0]["kind"], "workspace");
        assert_eq!(ports[0]["owner"]["worktreeId"], "repoA::/work/featureA");
        assert_eq!(ports[0]["owner"]["repoId"], "repoA");
        assert_eq!(ports[0]["owner"]["confidence"], "cwd");
        assert_eq!(ports[0]["connectHost"], "127.0.0.1");
        assert_eq!(ports[0]["pid"], 10);
    }

    #[test]
    fn longest_worktree_path_wins() {
        // A nested worktree under a parent repo: the deeper match owns the port.
        let worktrees = vec![
            worktree("repoA::/work/repo", "repoA", "Repo"),
            worktree("repoA::/work/repo/sub", "repoA", "Sub"),
        ];
        let listeners = parse_lsof_listeners("p11\ncnode\nn127.0.0.1:4000\n");
        let mut cwds = BTreeMap::new();
        cwds.insert(11u32, "/work/repo/sub/app".to_string());
        let ports = build_ports(&listeners, &cwds, &worktrees);
        assert_eq!(ports[0]["owner"]["displayName"], "Sub");
    }

    #[test]
    fn unattributed_listener_is_external() {
        let listeners = parse_lsof_listeners("p20\ncpostgres\nn127.0.0.1:5432\n");
        let cwds = BTreeMap::new();
        let ports = build_ports(&listeners, &cwds, &[]);
        assert_eq!(ports[0]["kind"], "external");
        assert!(ports[0].get("owner").is_none());
    }

    #[test]
    fn dedups_ipv4_ipv6_and_prefers_concrete_bind() {
        // Same pid+port on wildcard then loopback collapses to one, concrete bind.
        let listeners = parse_lsof_listeners("p30\ncnode\nn*:3000\nn127.0.0.1:3000\n");
        let cwds = BTreeMap::new();
        let ports = build_ports(&listeners, &cwds, &[]);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0]["bindHost"], "127.0.0.1");
        assert_eq!(ports[0]["connectHost"], "127.0.0.1");
    }

    #[test]
    fn wildcard_connect_host_normalizes_to_loopback() {
        let listeners = parse_lsof_listeners("p40\ncnode\nn*:8000\n");
        let ports = build_ports(&listeners, &BTreeMap::new(), &[]);
        assert_eq!(ports[0]["bindHost"], "*");
        assert_eq!(ports[0]["connectHost"], "127.0.0.1");
    }

    #[test]
    fn path_contains_is_component_wise() {
        assert!(path_contains("/a/foo", "/a/foo"));
        assert!(path_contains("/a/foo", "/a/foo/bar"));
        assert!(!path_contains("/a/foo", "/a/foobar"));
    }

    #[test]
    fn protocol_picks_https_for_tls_ports() {
        assert_eq!(protocol_for(443), "https");
        assert_eq!(protocol_for(8443), "https");
        assert_eq!(protocol_for(3000), "http");
    }

    #[test]
    fn repo_filter_drops_other_repos_attribution() {
        // Simulate the repo_id filter the command applies before build_ports.
        let all = vec![
            worktree("repoA::/work/a", "repoA", "A"),
            worktree("repoB::/work/b", "repoB", "B"),
        ];
        let filtered: Vec<_> = all.into_iter().filter(|wt| wt.repo_id == "repoA").collect();
        let listeners = parse_lsof_listeners("p50\ncnode\nn127.0.0.1:3000\n");
        let mut cwds = BTreeMap::new();
        cwds.insert(50u32, "/work/b/app".to_string());
        let ports = build_ports(&listeners, &cwds, &filtered);
        // The listener lives in repoB, which was filtered out -> external.
        assert_eq!(ports[0]["kind"], "external");
    }
}
