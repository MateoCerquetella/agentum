//! LIVE Phase-1 verification for spec 009a (host browser + forward tunnel).
//!
//! `#[ignore]` — it SSHes to a real host, launches a real headless Chromium in a
//! real tmux session, and opens an `ssh -L` tunnel, so it never runs in CI. Run:
//!
//!   cargo test -p agentum-server --test host_browser_live -- --ignored --nocapture
//!
//! Proves the Phase-1 acceptance: a headless Chromium runs ON the host, the Mac
//! forward-tunnels its CDP port, and `curl 127.0.0.1:<mac>/json/version` returns
//! the host Chromium's CDP banner — browser-on-host + forward tunnel end to end,
//! with zero UI.
//!
//! Credentials come from the environment — NEVER hardcode them (this file lives
//! under `crates/`, which is committable). The password is required; the test
//! soft-skips when it's unset. Host/user/port/workdir default to the Omarchy
//! test box but are overridable:
//!
//!   AGENTUM_LIVE_SSH_PASSWORD=… \
//!   [AGENTUM_LIVE_SSH_USER=malloc] [AGENTUM_LIVE_SSH_HOST=172.30.66.4] \
//!   [AGENTUM_LIVE_SSH_PORT=44444] [AGENTUM_LIVE_SSH_WORKDIR=/home/malloc] \
//!   cargo test -p agentum-server --test host_browser_live -- --ignored --nocapture
//!
//! The interactive ControlMaster is reused if already warm, so this needs no
//! fresh auth when the desktop/TUI already has the host open.

use std::path::PathBuf;
use std::process::Command;

use agentum_core::{Host, HostKind, SshAuth};
use agentum_server::{host_browser, host_runtime};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Build the live host from env. `None` (→ soft-skip) when the password isn't
/// set, so the secret never has to live in source and CI never trips on it.
fn live_host() -> Option<Host> {
    let password = std::env::var("AGENTUM_LIVE_SSH_PASSWORD").ok()?;
    Some(Host {
        id: agentum_core::LOCAL_HOST_ID,
        name: "live".into(),
        kind: HostKind::Ssh {
            user: env_or("AGENTUM_LIVE_SSH_USER", "malloc"),
            hostname: env_or("AGENTUM_LIVE_SSH_HOST", "172.30.66.4"),
            port: env_or("AGENTUM_LIVE_SSH_PORT", "44444")
                .parse()
                .expect("AGENTUM_LIVE_SSH_PORT must be a u16"),
            auth: SshAuth::Password { password },
        },
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
        last_seen_at: None,
    })
}

#[tokio::test]
#[ignore = "live: needs an SSH host + chromium on it; set AGENTUM_LIVE_SSH_PASSWORD"]
async fn host_browser_launch_tunnel_and_cdp_banner() {
    let Some(host) = live_host() else {
        eprintln!("skipping: AGENTUM_LIVE_SSH_PASSWORD not set");
        return;
    };
    // The workdir's basename is the worktree slug; default exists on the host.
    let workdir = PathBuf::from(env_or("AGENTUM_LIVE_SSH_WORKDIR", "/home/malloc"));
    let workdir = workdir.as_path();

    // Start from a clean slate so we exercise the fresh-launch path (not attach).
    let _ = host_browser::teardown_host_browser(&host, workdir).await;

    let browser = host_browser::launch_host_browser(&host, workdir)
        .await
        .expect("launch host browser");
    println!(
        "launched: target={} cdp_port={} user_data_dir={} attached={}",
        browser.tmux_target, browser.cdp_port, browser.user_data_dir, browser.attached
    );
    assert!(browser.cdp_port > 0, "Chromium bound no CDP port");

    let mac_port = host_runtime::ensure_forward_tunnel(&host, browser.cdp_port)
        .await
        .expect("forward tunnel");
    println!(
        "forward tunnel up: mac 127.0.0.1:{mac_port} -> host 127.0.0.1:{}",
        browser.cdp_port
    );

    // The actual Phase-1 proof: the host Chromium's CDP banner over the tunnel.
    let out = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "10",
            &format!("http://127.0.0.1:{mac_port}/json/version"),
        ])
        .output()
        .expect("run curl");
    let body = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("GET /json/version => {body}");

    // Teardown before asserting so a failed assertion still reaps the browser.
    let _ = host_browser::teardown_host_browser(&host, workdir).await;

    assert!(
        body.contains("webSocketDebuggerUrl") || body.contains("Chrome"),
        "CDP banner not returned through the forward tunnel: {body:?}"
    );
}
