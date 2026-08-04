use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // `agentum` is the terminal application. There is no subcommand layer:
    // launching the one installed binary enters the TUI directly.
    agentum::init_tracing_for_tui();
    agentum::commands::terminal::run(Default::default()).await
}
