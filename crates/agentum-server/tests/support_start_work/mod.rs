//! Shared driver for the start-work-leg LIVE tests (spec 008 F1 §B.3): the leg
//! `harness_live_agent.rs` skips — issue → `POST /api/harness/start-work` →
//! a real session opens → the spec/issue-grounded prompt lands. claude + tmux
//! stay REAL; only the GitHub fetch is stubbed (a fake `gh` via `AGENTUM_GH_BIN`
//! serving a canned issue), so the test never depends on a real issue #42.
//!
//! Included via `#[path]` into TWO thin binaries (roles-off + roles-on) rather
//! than one binary with two tests: each ends in `std::process::exit(0)` to dodge
//! the runtime-teardown hang the model `harness_live_agent.rs` documents, and a
//! single `exit(0)` would kill a sibling test in the same process.
#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use agentum_server::harness::HarnessEvent;
use agentum_server::serve_embedded_loopback_state;
use agentum_store::Store;
use tempfile::TempDir;
use tokio::time::timeout;

/// A distinctive token embedded in the canned issue's acceptance criteria; it
/// flows into the planned feature's name/description → the injected prompt, so
/// finding it in the agent's tmux pane proves the prompt landed (AC 2).
const MARKER: &str = "SPEC008WIDGETXYZ";

/// The roles knob key (a literal, so the route const need not be public).
const SDD_ROLES_ENABLED_SETTING: &str = "harness.sdd.roles.enabled";

fn write_exec(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Kill ONLY the tmux panes this run created (`agentum-harness-*-<short8>`),
/// never a user's own session — the discipline `harness_live_agent.rs` uses.
fn cleanup_panes(short: &str) {
    if let Ok(out) = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.starts_with("agentum-harness-") && line.ends_with(short) {
                let _ = std::process::Command::new("tmux")
                    .args(["kill-session", "-t", line])
                    .status();
            }
        }
    }
}

/// True once the injected prompt's `MARKER` is visible in ANY harness pane for
/// this run — the feature pane (roles off) or the PM-gate pane (roles on). Reads
/// the full scrollback (`capture-pane -p -S -`) so a marker that scrolled off the
/// visible region still counts.
async fn capture_pane_has_marker(short: &str) -> bool {
    let Ok(sessions) = tokio::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .await
    else {
        return false;
    };
    for line in String::from_utf8_lossy(&sessions.stdout).lines() {
        if !(line.starts_with("agentum-harness-") && line.ends_with(short)) {
            continue;
        }
        if let Ok(cap) = tokio::process::Command::new("tmux")
            .args(["capture-pane", "-t", line, "-p", "-S", "-"])
            .output()
            .await
        {
            if String::from_utf8_lossy(&cap.stdout).contains(MARKER) {
                return true;
            }
        }
    }
    false
}

/// Drive the whole leg once. `roles_on` selects the first spawn: OFF → the
/// FEATURE agent spawns first (deterministic, emits `AgentSpawned`); ON → the PM
/// role gate spawns first (spec 006 D1 default; its spawn is a `Log`, since a
/// role agent isn't feature-bound). Asserts a session opened, the prompt reached
/// its pane, and the issue's `status/*` labels flipped through the fake `gh`.
pub async fn run(roles_on: bool) {
    // --- Stub the GitHub fetch (a fake `gh`), keep claude + tmux real ---------
    let workspace = TempDir::new().unwrap();
    let ghdir = TempDir::new().unwrap();
    let log = ghdir.path().join("gh-calls.log");
    let issue_json_path = ghdir.path().join("issue.json");
    let slug = "acme/widgets";

    // The canned issue: two `- [ ]` boxes (≥1 → the transform plans a feature),
    // MARKER in the first. `\n` are JSON escapes (raw string → literal
    // backslash-n) that serde parses back to newlines, so the checkboxes land on
    // their own lines for the planner.
    // Delimiter is `r###"…"###`: the markdown `"##` headings contain `"#`, which
    // would terminate a plain `r#"…"#` raw string early.
    let issue_json = format!(
        r###"{{"title":"Add a widget","body":"## Problem\n\nNeed a widget.\n\n## Acceptance criteria\n\n- [ ] render the {marker} component\n- [ ] cover it with a test\n","url":"https://github.com/{slug}/issues/42"}}"###,
        marker = MARKER,
        slug = slug,
    );
    std::fs::write(&issue_json_path, &issue_json).unwrap();

    // `issue view` → the canned JSON on stdout; every other call (edit / label /
    // comment) is logged so we can assert the label flips.
    let gh = ghdir.path().join("gh-fake");
    write_exec(
        &gh,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"view\" ]; then\n  \
             cat \"{issue}\"\n  exit 0\nfi\necho \"$@\" >> \"{log}\"\nexit 0\n",
            issue = issue_json_path.display(),
            log = log.display(),
        ),
    );

    // The fetch (`host_runtime::gh_in_dir`) AND the transitions
    // (`task_sink::gh_bin`) both honor AGENTUM_GH_BIN. AGENTUM_GITHUB_CONFIG →
    // an absent file so a dev `github.json` can't rename the asserted labels.
    unsafe {
        std::env::set_var("AGENTUM_GH_BIN", &gh);
        std::env::set_var("AGENTUM_GITHUB_CONFIG", ghdir.path().join("github.json"));
    }

    // --- Boot the real embedded loopback server -------------------------------
    let db = workspace.path().join("agentum-test.sqlite");
    let store = Store::open(&db).await.expect("open store");
    store
        .setting_set_bool(SDD_ROLES_ENABLED_SETTING, roles_on)
        .await
        .expect("set roles knob");

    let (addr, state) = serve_embedded_loopback_state(store)
        .await
        .expect("boot embedded server");
    eprintln!("[live] embedded server on {addr}; roles_on={roles_on}");

    let mut rx = state.harness.subscribe();

    // --- Act: POST the start-work route (loopback = no auth) ------------------
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("http://{addr}/api/harness/start-work"))
        .json(&serde_json::json!({
            "workdir": workspace.path().to_string_lossy(),
            "number": "42",
            "slug": slug,
            "agentTool": "claude",
        }))
        .send()
        .await
        .expect("start-work request")
        .json()
        .await
        .expect("start-work json");
    eprintln!("[live] start-work response: {resp}");
    assert_eq!(
        resp["runStarted"].as_bool(),
        Some(true),
        "start-work must report runStarted, got: {resp}"
    );
    let harness_id = resp["harnessId"]
        .as_str()
        .expect("harnessId in the start-work response")
        .to_string();
    // The tmux session suffix is the first 8 hex of the harness id's simple form.
    let simple = harness_id.replace('-', "");
    let short = &simple[..8];

    // --- Watch: a spawn signal + the prompt reaching the pane -----------------
    let mut saw_spawn = false;
    let mut prompt_landed = false;
    let short_owned = short.to_string();
    let watch = async {
        loop {
            if saw_spawn && prompt_landed {
                break;
            }
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(ev) => {
                        eprintln!("[live][event] {ev:?}");
                        match &ev {
                            HarnessEvent::AgentSpawned { .. } => saw_spawn = true,
                            HarnessEvent::Log { message, .. } => {
                                // Roles ON: the FEATURE agent's AgentSpawned only
                                // fires after the PM/architect/decompose gates, so
                                // accept the PM-gate spawn log as "a session opened".
                                if roles_on
                                    && message.contains("spawning")
                                    && message.contains("pm")
                                {
                                    saw_spawn = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    if !prompt_landed && capture_pane_has_marker(&short_owned).await {
                        prompt_landed = true;
                    }
                }
            }
        }
    };
    // Bounded so a stuck agent can never hang the suite.
    let _ = timeout(Duration::from_secs(240), watch).await;
    // One last poll in case the marker appeared right at the deadline.
    if !prompt_landed {
        prompt_landed = capture_pane_has_marker(short).await;
    }

    let gh_log = std::fs::read_to_string(&log).unwrap_or_default();
    eprintln!("[live] gh calls:\n{gh_log}");
    cleanup_panes(short);
    unsafe {
        std::env::remove_var("AGENTUM_GH_BIN");
        std::env::remove_var("AGENTUM_GITHUB_CONFIG");
    }

    eprintln!("[live] saw_spawn={saw_spawn} prompt_landed={prompt_landed}");
    assert!(
        saw_spawn,
        "start-work must open a real agent session (AC 1)"
    );
    assert!(
        prompt_landed,
        "the injected prompt (marker {MARKER}) must reach the agent's pane (AC 2)"
    );
    // The plan fires Todo for both variants; roles-off also flips InProgress at
    // the feature spawn (roles-on reaches that only after the role gates).
    assert!(
        gh_log.contains("--add-label status/todo"),
        "the plan must flip the issue to status/todo, got:\n{gh_log}"
    );
    if !roles_on {
        assert!(
            gh_log.contains("--add-label status/in-progress"),
            "the feature spawn must flip the issue to status/in-progress, got:\n{gh_log}"
        );
    }
}
