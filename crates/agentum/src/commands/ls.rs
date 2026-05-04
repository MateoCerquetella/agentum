use agentum_core::Status;
use anyhow::Result;

pub async fn run(running_only: bool, tool_filter: Option<String>) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let filter = if running_only { Some(Status::Running) } else { None };
    let mut sessions = store.list_sessions(filter).await?;

    if let Some(ref tool) = tool_filter {
        sessions.retain(|s| s.tool == *tool);
    }

    if sessions.is_empty() {
        println!("(no sessions)");
        return Ok(());
    }

    let name_w = sessions.iter().map(|s| s.name.len()).max().unwrap_or(8).max(4);
    let tool_w = sessions.iter().map(|s| s.tool.len()).max().unwrap_or(6).max(4);

    println!(
        "{:<nw$}  {:<7}  {:<tw$}  workdir",
        "NAME", "STATUS", "TOOL", nw = name_w, tw = tool_w
    );
    for s in sessions {
        println!(
            "{:<nw$}  {:<7}  {:<tw$}  {}",
            s.name, s.status, s.tool, s.workdir,
            nw = name_w, tw = tool_w
        );
    }
    Ok(())
}
