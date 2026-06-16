use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = agentum::cli::Cli::parse();
    // The TUI subcommand owns the alt-screen; tracing to stderr would land
    // log lines on top of ratatui's draw and (especially inside tmux) leave
    // the screen completely black. Route to a file in that mode only.
    if matches!(cli.command, agentum::cli::Cmd::Terminal { .. }) {
        agentum::init_tracing_for_tui();
    } else {
        agentum::init_tracing();
    }
    agentum::cli::dispatch(cli).await
}
