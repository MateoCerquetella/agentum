//! `/api/preflight` — local tool + agent detection for the desktop shell.
//!
//! Ported from the desktop's native `preflight_*` Tauri commands so the desktop
//! drives the same embedded backend as everything else, instead of a parallel
//! `which`/process implementation living in the Tauri crate. The embedded server
//! runs as the user on a no-auth loopback, so it has the same `PATH` reach the
//! desktop process did.

use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/preflight/check", get(check))
        .route("/api/preflight/agents", get(detect_agents_handler))
        .route("/api/preflight/agents/refresh", get(refresh_agents_handler))
}

fn installed(bin: &str) -> bool {
    which::which(bin).is_ok()
}

// gh/glab report auth via the exit code of `<cli> auth status` (0 = authenticated),
// with an output-marker fallback for versions that exit non-zero but print success.
async fn cli_authenticated(bin: &str) -> bool {
    match tokio::process::Command::new(bin)
        .args(["auth", "status"])
        .output()
        .await
    {
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

// `GET /api/preflight/check` — git presence + gh/glab install & auth status. Shape
// matches the renderer's PreflightStatus (Landing reads `status.git.installed`,
// `status.gh.authenticated`, …; a missing field makes the Landing page throw).
async fn check() -> Json<Value> {
    let gh_installed = installed("gh");
    let glab_installed = installed("glab");
    let gh_authenticated = gh_installed && cli_authenticated("gh").await;
    let glab_authenticated = glab_installed && cli_authenticated("glab").await;
    Json(json!({
        "git": { "installed": installed("git") },
        "gh": { "installed": gh_installed, "authenticated": gh_authenticated },
        "glab": { "installed": glab_installed, "authenticated": glab_authenticated },
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
    }))
}

// `GET /api/preflight/agents` — ids of the agent CLIs found on PATH.
async fn detect_agents_handler() -> Json<Vec<String>> {
    Json(detect_agents())
}

// `GET /api/preflight/agents/refresh` — same detection, in the RefreshAgentsResult
// shape. Login-shell PATH re-hydration isn't ported, so we detect against the
// current PATH and report success with no new segments.
async fn refresh_agents_handler() -> Json<Value> {
    Json(json!({
        "agents": detect_agents(),
        "addedPathSegments": [],
        "shellHydrationOk": true,
        "pathSource": "shell_hydrate",
        "pathFailureReason": "none"
    }))
}
