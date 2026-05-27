use agentum_store::paths;
use anyhow::{Result, bail};

pub async fn run(name: String, lines: u32, follow: bool) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };

    let log = paths::pane_log(&session.id.to_string())?;
    if !log.exists() {
        bail!(
            "no log file for session {name} (has it been started?)\n  expected: {}",
            log.display()
        );
    }

    // Delegate to system `tail` — it handles -f efficiently with inotify/kqueue
    // and avoids us re-implementing line counting + follow in Rust.
    let mut cmd = tokio::process::Command::new("tail");
    cmd.arg("-n").arg(lines.to_string());
    if follow {
        cmd.arg("-f");
    }
    cmd.arg(&log);

    let status = cmd.status().await?;
    std::process::exit(status.code().unwrap_or(1));
}
