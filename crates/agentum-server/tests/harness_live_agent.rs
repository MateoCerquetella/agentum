//! LIVE end-to-end test: the Harness Engine drives a REAL agent (Claude Code)
//! through the verification gate against a copy of `examples/harness-demo/`.
//!
//! `#[ignore]` — this spawns a real `claude` CLI in a real tmux pane, consumes
//! tokens, and takes minutes, so it never runs in CI. Run it explicitly:
//!
//!   AGENTUM_BROWSER_VERIFY=1 \
//!   cargo test -p agentum-server --test harness_live_agent -- --ignored --nocapture
//!
//! It boots the SAME embedded loopback server the desktop/TUI use
//! (`serve_embedded_loopback_state`), registers the demo as a harness, kicks off
//! `harness::drive`, and watches the real `HarnessEvent` stream until the gate
//! verifies the agent's work. Asserts the agent actually created `GREETING.md`
//! and that `verify.sh` passed for the first feature.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agentum_server::harness::HarnessEvent;
use agentum_store::Store;
use tempfile::TempDir;
use tokio::time::timeout;

fn demo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/harness-demo")
        .canonicalize()
        .expect("examples/harness-demo must exist")
}

/// Copy the demo's `.harness/` into a fresh temp project so the agent's edits
/// (GREETING.md, rewritten feature_list.json, handoff.md) never touch the repo.
/// The settle timeout is trimmed so a stuck agent fails fast instead of pinning
/// the gate for the demo's 900s.
fn stage_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = demo_dir().join(".harness");
    let dst = tmp.path().join(".harness");
    std::fs::create_dir_all(&dst).unwrap();
    for name in ["AGENTS.md", "init.sh", "verify.sh", "handoff.md"] {
        std::fs::copy(src.join(name), dst.join(name)).unwrap();
    }
    // Rewrite feature_list.json with a bounded settle timeout for the test.
    let raw = std::fs::read_to_string(src.join("feature_list.json")).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    json["settle_timeout_secs"] = serde_json::json!(240);
    json["agent_yolo"] = serde_json::json!(true);
    std::fs::write(
        dst.join("feature_list.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    tmp
}

/// Best-effort: kill ONLY the tmux panes THIS run created. The harness names
/// panes `agentum-harness-<feature>-<short>` where `<short>` is the first 8 hex
/// of the harness id — matching on that suffix avoids ever touching an unrelated
/// session like the user's own `agentum-harness-engine`.
fn cleanup_panes(harness_id: &uuid::Uuid) {
    let short = harness_id.simple().to_string()[..8].to_string();
    if let Ok(out) = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.starts_with("agentum-harness-") && line.ends_with(&short) {
                let _ = std::process::Command::new("tmux")
                    .args(["kill-session", "-t", line])
                    .status();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real claude agent; run with --ignored"]
async fn harness_drives_a_real_agent_through_the_gate() {
    // The task asked for the browser-verify MCP wiring to be active.
    unsafe { std::env::set_var("AGENTUM_BROWSER_VERIFY", "1") };

    let project = stage_project();
    let workdir = project.path().to_path_buf();
    let greeting = workdir.join("GREETING.md");
    assert!(!greeting.exists(), "GREETING.md must not exist yet");

    // Boot the real embedded server (watchdog + harness engine + workers).
    let db = workdir.join("agentum-test.sqlite");
    let store = Store::open(&db).await.expect("open store");
    let (addr, state) = agentum_server::serve_embedded_loopback_state(store)
        .await
        .expect("boot embedded server");
    eprintln!("[live] embedded server on {addr}");

    let id = state
        .harness
        .start(workdir.clone())
        .await
        .expect("register harness");
    eprintln!("[live] harness {id} registered for {}", workdir.display());

    let mut rx = state.harness.subscribe();

    // Kick the drive loop off exactly like POST /{id}/run does.
    let driver = tokio::spawn(agentum_server::harness::drive(state.clone(), id));

    // Watch the live event stream. Evidence is mirrored to a file so it survives
    // regardless of test-output buffering.
    let mut log = String::new();
    let mut hello_verified = false;
    let mut saw_spawn = false;
    let mut completed = false;

    let watch = async {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let line = format!("{ev:?}");
                    eprintln!("[live][event] {line}");
                    log.push_str(&line);
                    log.push('\n');
                    match &ev {
                        HarnessEvent::AgentSpawned { feature_id, .. } => {
                            if feature_id == "hello-file" {
                                saw_spawn = true;
                            }
                        }
                        HarnessEvent::VerifyCompleted {
                            feature_id,
                            success,
                            ..
                        } => {
                            if feature_id == "hello-file" && *success {
                                hello_verified = true;
                            }
                        }
                        HarnessEvent::HarnessCompleted { success, .. } => {
                            completed = *success;
                            break;
                        }
                        HarnessEvent::StateChanged { state, .. } => {
                            use agentum_server::harness::HarnessState::*;
                            if matches!(state, Blocked | Failed) {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // Overall ceiling so a hung agent can never hang the suite.
    let _ = timeout(Duration::from_secs(600), watch).await;
    driver.abort();
    let _ = std::fs::write("/tmp/harness_live_events.log", &log);
    cleanup_panes(&id);

    eprintln!("[live] saw_spawn={saw_spawn} hello_verified={hello_verified} completed={completed}");
    eprintln!("[live] GREETING.md exists: {}", greeting.exists());
    if let Ok(body) = std::fs::read_to_string(&greeting) {
        eprintln!("[live] GREETING.md:\n{body}");
    }

    assert!(
        saw_spawn,
        "the harness should have spawned a real agent for hello-file"
    );
    assert!(
        greeting.exists(),
        "the real agent should have created GREETING.md"
    );
    let body = std::fs::read_to_string(&greeting).unwrap_or_default();
    assert!(
        body.lines().next() == Some("Hello from the Agentum Harness Engine"),
        "GREETING.md first line must be the required greeting, got:\n{body}"
    );
    assert!(
        hello_verified,
        "verify.sh must have passed (green gate) for hello-file"
    );

    // The embedded server's background workers (watchdog, axum serve) keep the
    // tokio runtime busy, so a natural return would hang on runtime teardown.
    // All assertions passed — exit cleanly with success so the run is
    // deterministic and reproducible.
    std::process::exit(0);
}
