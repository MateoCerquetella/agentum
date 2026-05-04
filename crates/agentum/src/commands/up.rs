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

    // If the DB says running and tmux confirms, we're done.
    if matches!(session.status, Status::Running)
        && agentum_tmux::has_session(&target).await?
    {
        println!("{name} already running  → tmux:{target}");
        return Ok(());
    }

    // Refuse to clobber a stranger holding the same tmux name.
    if agentum_tmux::has_session(&target).await? {
        bail!("tmux session {target} already exists outside agentum; refuse to clobber");
    }

    let workdir = PathBuf::from(&session.workdir);
    if !workdir.exists() {
        bail!("workdir does not exist: {}", workdir.display());
    }

    let mut cmd = vec![session.tool.clone()];
    if let Some(model) = &session.model {
        cmd.push(format!("--model={model}"));
    }
    cmd.extend(session.flags.clone());

    agentum_tmux::new_session(&target, &workdir, &cmd, &[]).await?;

    let log = paths::pane_log(&session.id.to_string())?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        // Pipe failure should not orphan the tmux session.
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(e.into());
    }

    store
        .update_status_and_target(session.id, Status::Running, Some(&target))
        .await?;
    println!("up          {name}  → tmux:{target}  log:{}", log.display());
    Ok(())
}
