//! Ignored end-to-end smoke test for the two SSH launch paths most likely to
//! regress: a plain remote shell and Codex with an authenticated HTTP MCP.
//!
//! Credentials are environment-only and are never printed. Run explicitly:
//!
//! ```text
//! AGENTUM_LIVE_SSH_AUTH=agent \
//! AGENTUM_LIVE_SSH_USER=... \
//! AGENTUM_LIVE_SSH_HOST=... \
//! AGENTUM_LIVE_SSH_WORKDIR=/absolute/remote/path \
//! cargo test -p agentum-server --test ssh_agents_live -- --ignored --nocapture
//! ```
//!
//! Set `AGENTUM_LIVE_SSH_AUTH=password` (the default) together with
//! `AGENTUM_LIVE_SSH_PASSWORD` to exercise password/askpass authentication.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentum_core::{Host, HostKind, Session, SshAuth, Status};
use agentum_executor::{McpProvision, McpServer, RemoteLaunchContext};
use agentum_server::host_runtime;
use agentum_tmux::ssh::{SshMux, ssh_control_exit_cmd};
use time::OffsetDateTime;
use uuid::Uuid;

fn required_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn live_target() -> Option<(Host, PathBuf)> {
    let user = required_env("AGENTUM_LIVE_SSH_USER")?;
    let hostname = required_env("AGENTUM_LIVE_SSH_HOST")?;
    let workdir = PathBuf::from(required_env("AGENTUM_LIVE_SSH_WORKDIR")?);
    let auth = match std::env::var("AGENTUM_LIVE_SSH_AUTH").as_deref() {
        Ok("agent") => SshAuth::Agent,
        Ok("password") | Err(_) => SshAuth::Password {
            password: required_env("AGENTUM_LIVE_SSH_PASSWORD")?,
        },
        Ok(_) => return None,
    };
    let port = std::env::var("AGENTUM_LIVE_SSH_PORT")
        .unwrap_or_else(|_| "22".to_string())
        .parse()
        .ok()?;
    let now = OffsetDateTime::now_utc();
    Some((
        Host {
            id: Uuid::new_v4(),
            name: "live-ssh-agent-smoke".into(),
            kind: HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            },
            created_at: now,
            updated_at: now,
            last_seen_at: None,
        },
        workdir,
    ))
}

fn session(id: Uuid, workdir: &Path, tool: &str) -> Session {
    let now = OffsetDateTime::now_utc();
    Session {
        id,
        name: format!("live-{tool}-{}", &id.simple().to_string()[..8]),
        workdir: workdir.display().to_string(),
        tool: tool.into(),
        model: None,
        flags: Vec::new(),
        status: Status::Idle,
        tmux_target: None,
        host_id: None,
        host_label: None,
        host_kind: Some("ssh".into()),
        created_at: now,
        updated_at: now,
        last_activity_at: None,
        tokens: None,
        cost_usd: None,
        ctx: None,
        last_log: None,
        uptime_seconds: None,
        state: None,
        pinned: false,
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    }
}

async fn session_names(host: &Host) -> HashSet<String> {
    host_runtime::list_all_tmux_sessions(host)
        .await
        .expect("list remote tmux sessions")
        .into_iter()
        .map(|session| session.name)
        .collect()
}

async fn close_smoke_control_masters(host: &Host) {
    // Close only this test's UUID/revision-namespaced masters. Deliberately do
    // not call the broad upgrade-retirement helper: a developer may be running
    // another Agentum process against the same endpoint during this smoke.
    for mux in [SshMux::Interactive, SshMux::Streaming] {
        if let Some(mut command) = ssh_control_exit_cmd(host, mux) {
            let _ = tokio::time::timeout(Duration::from_secs(3), command.output()).await;
        }
    }
}

#[tokio::test]
#[ignore = "live: requires AGENTUM_LIVE_SSH_{AUTH,USER,HOST,WORKDIR}"]
async fn remote_terminal_and_codex_authenticated_mcp_launch() {
    let Some((host, requested_workdir)) = live_target() else {
        eprintln!("skipping: live SSH environment is incomplete");
        return;
    };
    let before = session_names(&host).await;
    let scratch = tempfile::tempdir().expect("local pane-log scratch dir");

    // Plain terminal must use the remote account's actual shell, never the
    // daemon machine's `$SHELL`.
    let terminal_id = Uuid::new_v4();
    let terminal_target = format!("agentum-live-terminal-{}", terminal_id.simple());
    let terminal_log = scratch.path().join(format!("{terminal_id}.log"));
    let terminal_preflight =
        host_runtime::preflight_remote_launch(&host, &requested_workdir, "terminal", terminal_id)
            .await
            .expect("remote terminal preflight");
    assert_eq!(terminal_preflight.executable, terminal_preflight.shell);
    host_runtime::launch_remote_session(
        &host,
        &terminal_target,
        &terminal_preflight.workdir,
        std::slice::from_ref(&terminal_preflight.executable),
        &[("AGENTUM_LIVE_SMOKE".into(), "terminal".into())],
        &[],
        &terminal_log,
    )
    .await
    .expect("launch remote terminal");
    host_runtime::send_keys(
        &host,
        &terminal_target,
        "printf 'AGENTUM_REMOTE_TERMINAL_OK\\n'",
        true,
    )
    .await
    .expect("write to remote terminal");
    let mut terminal_output = String::new();
    for _ in 0..20 {
        terminal_output = host_runtime::capture_pane_visible(&host, &terminal_target)
            .await
            .unwrap_or_default();
        if terminal_output.contains("AGENTUM_REMOTE_TERMINAL_OK") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let terminal_cleanup = host_runtime::kill_session(&host, &terminal_target).await;
    assert!(
        terminal_output.contains("AGENTUM_REMOTE_TERMINAL_OK"),
        "remote shell did not execute an interactive command"
    );
    terminal_cleanup.expect("clean up remote terminal");

    // Codex must receive a supported `bearer_token_env_var` reference while
    // the bearer itself stays exclusively in the staged child environment.
    let codex_id = Uuid::new_v4();
    let codex_session = session(codex_id, &requested_workdir, "codex");
    let codex_target = format!("agentum-live-codex-{}", codex_id.simple());
    let codex_log = scratch.path().join(format!("{codex_id}.log"));
    let codex_preflight =
        host_runtime::preflight_remote_launch(&host, &requested_workdir, "codex", codex_id)
            .await
            .expect("remote Codex preflight");
    let env_name = format!(
        "AGENTUM_MCP_AUTH_{}",
        Uuid::new_v4().simple().to_string().to_ascii_uppercase()
    );
    let token = format!("live-scoped-smoke-{}", Uuid::new_v4().simple());
    let provision = McpProvision {
        servers: vec![McpServer {
            name: "agentum".into(),
            // The endpoint need not answer for this parser/startup smoke; an
            // unreachable MCP may warn, but a valid Codex TUI remains alive.
            url: "http://127.0.0.1:9/mcp".into(),
            auth_token: Some(token.clone()),
            auth_env_var: Some(env_name.clone()),
        }],
        config_file: PathBuf::new(),
    };
    let adapter = agentum_executor::adapter_for("codex");
    let mut launch = adapter.launch_remote(
        &codex_session,
        &RemoteLaunchContext {
            shell: codex_preflight.shell.as_str().into(),
            claude_transcript_exists: false,
        },
    );
    launch.argv[0] = codex_preflight.executable.clone();
    adapter.apply_mcp(&mut launch, &provision);
    assert!(
        launch
            .argv
            .iter()
            .any(|arg| arg.contains("bearer_token_env_var") && arg.contains(&env_name))
    );
    assert!(!launch.argv.iter().any(|arg| arg.contains(&token)));
    assert!(
        launch
            .env
            .iter()
            .any(|(name, value)| name == &env_name && value == &token)
    );

    let codex_result = host_runtime::launch_remote_session(
        &host,
        &codex_target,
        &codex_preflight.workdir,
        &launch.argv,
        &launch.env,
        &[token.as_str()],
        &codex_log,
    )
    .await;
    let codex_cleanup = host_runtime::kill_session(&host, &codex_target).await;
    codex_result.expect("Codex accepted authenticated HTTP MCP configuration and stayed alive");
    codex_cleanup.expect("clean up remote Codex");

    let after = session_names(&host).await;
    let missing: Vec<_> = before
        .into_iter()
        .filter(|existing| !after.contains(existing))
        .collect();
    close_smoke_control_masters(&host).await;
    assert!(
        missing.is_empty(),
        "live smoke removed pre-existing tmux sessions: {missing:?}"
    );
}
