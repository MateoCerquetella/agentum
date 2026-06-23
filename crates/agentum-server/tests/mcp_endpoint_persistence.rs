//! Server-level proof of `MCP_CONNECTION_PERSISTENCE_PRD` R1 (token) + R2 (port):
//! the embedded loopback endpoint stays identical across a simulated desktop
//! restart, so an agent session's baked-in MCP config (URL + bearer token) keeps
//! working with zero user action.

use agentum_server::endpoint;
use tempfile::TempDir;

#[tokio::test]
async fn embedded_endpoint_is_stable_across_restart() {
    let home = TempDir::new().unwrap();
    // AGENTUM_HOME redirects state_dir() at the persisted files. This is the only
    // test in this binary, so the process-global env write can't race other tests.
    unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };

    // Boot 1: bind the preferred/default port and mint + persist the /mcp token.
    let l1 = endpoint::bind_stable_loopback().await.unwrap();
    let port1 = l1.local_addr().unwrap().port();
    let token1 = endpoint::load_or_create_mcp_token();
    assert_ne!(port1, 0, "should bind a concrete port");

    // The server process goes away on restart — release the listening socket.
    drop(l1);

    // Boot 2 (simulated restart): must reuse the same port AND the same token.
    let l2 = endpoint::bind_stable_loopback().await.unwrap();
    let port2 = l2.local_addr().unwrap().port();
    let token2 = endpoint::load_or_create_mcp_token();

    assert_eq!(port1, port2, "restart must reuse the persisted port");
    assert_eq!(token1, token2, "restart must reuse the persisted /mcp token");
}
