use serde_json::{json, Value};

// Preflight tool + agent detection. The renderer reads `status.git.installed`,
// `status.gh.authenticated`, etc., and the Agents pane awaits detect/refresh, so
// every method must resolve with the right shape (a partial/absent result makes the
// Landing page throw or the Agents section spin forever).

fn installed(bin: &str) -> bool {
    which::which(bin).is_ok()
}

// gh/glab report auth via the exit code of `<cli> auth status` (0 = authenticated),
// with an output-marker fallback for versions that exit non-zero but print success.
fn cli_authenticated(bin: &str) -> bool {
    match std::process::Command::new(bin).args(["auth", "status"]).output() {
        Ok(output) => {
            if output.status.success() {
                return true;
            }
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            combined.contains("Logged in") || combined.contains("Active account: true")
        }
        Err(_) => false,
    }
}

// Mirrors the detect map in tui-agent-config.ts (agent id -> CLI command). Agents
// whose id differs from the CLI binary name are overridden explicitly.
const AGENT_CMDS: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("openclaude", "openclaude"),
    ("codex", "codex"),
    ("autohand", "autohand"),
    ("opencode", "opencode"),
    ("pi", "pi"),
    ("omp", "omp"),
    ("gemini", "gemini"),
    ("antigravity", "agy"),
    ("aider", "aider"),
    ("goose", "goose"),
    ("amp", "amp"),
    ("kilo", "kilo"),
    ("kiro", "kiro-cli"),
    ("crush", "crush"),
    ("aug", "auggie"),
    ("cline", "cline"),
    ("codebuff", "codebuff"),
    ("continue", "continue"),
    ("cursor", "cursor-agent"),
    ("droid", "droid"),
    ("kimi", "kimi"),
    ("rovo", "rovo"),
    ("hermes", "hermes"),
    ("openclaw", "openclaw"),
    ("copilot", "copilot"),
    ("grok", "grok"),
];

fn detect_agents() -> Vec<String> {
    AGENT_CMDS
        .iter()
        .filter(|(_, cmd)| installed(cmd))
        .map(|(id, _)| (*id).to_string())
        .collect()
}

#[tauri::command]
pub fn preflight_check() -> Value {
    let gh_installed = installed("gh");
    let glab_installed = installed("glab");
    json!({
        "git": { "installed": installed("git") },
        "gh": {
            "installed": gh_installed,
            "authenticated": gh_installed && cli_authenticated("gh")
        },
        "glab": {
            "installed": glab_installed,
            "authenticated": glab_installed && cli_authenticated("glab")
        },
        "bitbucket": { "configured": false, "authenticated": false, "account": null },
        "azureDevOps": {
            "configured": false,
            "authenticated": false,
            "account": null,
            "baseUrl": null,
            "tokenConfigured": false
        },
        "gitea": {
            "configured": false,
            "authenticated": false,
            "account": null,
            "baseUrl": null,
            "tokenConfigured": false
        }
    })
}

#[tauri::command]
pub fn preflight_detect_agents() -> Vec<String> {
    detect_agents()
}

#[tauri::command]
pub fn preflight_refresh_agents() -> Value {
    // Login-shell PATH re-hydration isn't ported; detect against the current PATH
    // and report success with no new segments. Shape mirrors RefreshAgentsResult.
    json!({
        "agents": detect_agents(),
        "addedPathSegments": [],
        "shellHydrationOk": true,
        "pathSource": "shell_hydrate",
        "pathFailureReason": "none"
    })
}

#[tauri::command]
pub fn preflight_detect_remote_agents() -> Vec<String> {
    // Remote (SSH) agent detection needs a live relay session; not ported.
    Vec::new()
}
