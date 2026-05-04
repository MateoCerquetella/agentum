use std::net::SocketAddr;

use agentum_server::ServeOptions;
use anyhow::Result;

pub async fn run(addr: SocketAddr, cert_addr: SocketAddr, tls: bool) -> Result<()> {
    let (store, db_path) = super::open_store().await?;
    tracing::info!(?db_path, %addr, %cert_addr, tls, "store opened");
    agentum_server::serve(
        ServeOptions {
            addr,
            cert_addr,
            tls,
        },
        store,
    )
    .await
}
