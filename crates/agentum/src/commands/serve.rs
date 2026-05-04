use std::net::SocketAddr;

use anyhow::Result;

pub async fn run(addr: SocketAddr) -> Result<()> {
    let (store, db_path) = super::open_store().await?;
    tracing::info!(?db_path, "store opened");
    agentum_server::serve(addr, store).await
}
