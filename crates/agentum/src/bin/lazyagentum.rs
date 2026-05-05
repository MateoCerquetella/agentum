//! `lazyagentum` — short-circuit binary that drops you straight into the
//! agentum terminal dashboard, the same way `lazygit` lands you in the
//! lazygit UI without subcommands. Equivalent to `agentum terminal` but
//! with no other commands attached.

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "lazyagentum",
    version,
    about = "Drop into the agentum terminal dashboard with one keystroke.",
    long_about = "Equivalent to `agentum terminal`. Connects to a running \
                  `agentum serve` daemon and opens the dashboard. Use \
                  `--api` to point at a non-default daemon URL."
)]
struct Args {
    /// Override API base URL (defaults to https://127.0.0.1:8822 → http fallback).
    #[arg(long)]
    api: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    agentum::init_tracing();
    let args = Args::parse();
    agentum::commands::terminal::run(args.api).await
}
