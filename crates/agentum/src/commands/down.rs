use agentum_core::Status;
use anyhow::Result;

pub async fn run(name: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };
    if matches!(session.status, Status::Stopped | Status::Idle) {
        println!("{name} already down ({})", session.status);
        return Ok(());
    }
    store.update_status(session.id, Status::Stopped).await?;
    println!("down        {name}");
    Ok(())
}
