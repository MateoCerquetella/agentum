use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = cli::Cli::parse();
    cli::dispatch(cli).await
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("AGENTUM_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,h2=warn,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
