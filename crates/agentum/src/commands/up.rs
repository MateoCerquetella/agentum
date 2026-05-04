use agentum_core::Status;
use anyhow::Result;

pub async fn run(name: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };
    if matches!(session.status, Status::Running) {
        println!("{name} already running");
        return Ok(());
    }
    store.update_status(session.id, Status::Running).await?;
    println!("up          {name}  (phase 1: status only — tmux lands phase 2)");
    Ok(())
}
