use agentum_core::Status;
use anyhow::{Result, bail};

pub async fn run(name: String, text: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };

    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));

    if !matches!(session.status, Status::Running)
        || !agentum_tmux::has_session(&target).await?
    {
        bail!("session {name} is not running");
    }

    agentum_tmux::send_keys(&target, &text, true).await?;
    println!("sent        {name}  \"{text}\"");
    Ok(())
}
