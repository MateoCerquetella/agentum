//! The Code section: machines (agentd on each), coding threads and their events.
use crate::state::App;

/// For the Secretary's code_status tool: open threads, one line each.
pub async fn status_text(_app: &App) -> String {
    "No open coding threads.".into()
}

/// For the Secretary's code_task tool: start a thread in a project on the main machine.
pub async fn start_task(_app: &App, project: &str, _task: &str) -> anyhow::Result<String> {
    anyhow::bail!("No project called {project:?} yet")
}
