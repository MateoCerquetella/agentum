use agentum_core::Status;
use anyhow::Result;

pub async fn run(name: String, force: bool) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };

    if matches!(session.status, Status::Running) {
        if !force {
            eprintln!("session {name} is running; use --force to kill and remove");
            std::process::exit(1);
        }
        // --force: kill the tmux session first, then delete.
        let target = session
            .tmux_target
            .clone()
            .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
        agentum_tmux::kill_session(&target).await?;
    }

    store.delete_session(session.id).await?;
    println!("removed     {name}");
    Ok(())
}
