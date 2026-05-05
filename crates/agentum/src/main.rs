use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    agentum::init_tracing();
    let cli = agentum::cli::Cli::parse();
    agentum::cli::dispatch(cli).await
}
