use agentum_core::Status;
use anyhow::Result;

pub async fn run(name: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };

    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));

    agentum_tmux::kill_session(&target).await?;
    store
        .update_status_and_target(session.id, Status::Stopped, None)
        .await?;
    println!("killed      {name}");
    Ok(())
}
