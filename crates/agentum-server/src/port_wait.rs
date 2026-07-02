//! Shared TCP port-readiness helpers for the browser / MCP launch paths.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

/// A plain TCP connect is enough to know "something is serving here"; the
/// MCP/CDP client performs the protocol handshake itself. Short timeout so a
/// dead port fails fast on the launch hot-path.
pub(crate) async fn port_listening(port: u16) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Poll the port until it accepts connections or the deadline passes.
pub(crate) async fn wait_until_listening(port: u16, max: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        if port_listening(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
