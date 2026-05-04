use std::path::PathBuf;

use agentum_core::Status;
use agentum_store::paths;
use anyhow::{Result, bail};

pub async fn run(name: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };

    let target = agentum_tmux::target_for(&session.name);

    if matches!(session.status, Status::Running)
        && agentum_tmux::has_session(&target).await?
    {
        println!("{name} already running  → tmux:{target}");
        return Ok(());
    }

    if agentum_tmux::has_session(&target).await? {
        bail!("tmux session {target} already exists outside agentum; refuse to clobber");
    }

    let workdir = PathBuf::from(&session.workdir);
    if !workdir.exists() {
        bail!("workdir does not exist: {}", workdir.display());
    }

    // Dispatch through the executor abstraction. Unknown tools transparently
    // get a PassthroughAdapter — same shape as the old direct construction.
    let adapter = agentum_executor::adapter_for(&session.tool);
    let launch = adapter.launch(&session);
    tracing::debug!(
        tool = session.tool,
        adapter = adapter.name(),
        argv = ?launch.argv,
        "spawning tmux session"
    );

    agentum_tmux::new_session(&target, &workdir, &launch.argv, &launch.env).await?;

    let log = paths::pane_log(&session.id.to_string())?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(e.into());
    }

    store
        .update_status_and_target(session.id, Status::Running, Some(&target))
        .await?;
    println!(
        "up          {name}  → tmux:{target}  via:{}  log:{}",
        adapter.name(),
        log.display()
    );
    Ok(())
}
