//! Live boot smoke: stand up a REAL embedded agentum server (no-auth loopback)
//! and confirm the core routes respond. This is the literal "does the server
//! still boot and serve HTTP" check behind the module-extraction refactors that
//! split `routes/sessions.rs` (→ streaming/provision), `harness.rs`,
//! `cdp_driver.rs`, and `watchdog` — none of which is exercised by the unit
//! suite's in-process route helpers.
//!
//! `#[ignore]` by repo convention (it binds a socket, like `harness_mcp_e2e`);
//! run it explicitly to live-test the boot:
//!
//!   cargo test -p agentum-server --test server_boot_smoke -- --ignored --nocapture

use agentum_server::serve_embedded_loopback_state;
use agentum_store::Store;
use serde_json::Value;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "binds a real loopback socket; run explicitly: -- --ignored --nocapture"]
async fn embedded_server_boots_and_core_routes_respond() {
    // A real embedded server (no-auth loopback) on an ephemeral port — the same
    // boot path the desktop and TUI use in production.
    let db = TempDir::new().unwrap();
    let store = Store::open(&db.path().join("t.db")).await.unwrap();
    let (addr, _state) = serve_embedded_loopback_state(store).await.unwrap();
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();
    println!("\n================ agentum live at {base} ================\n");

    // Public, no auth — the bootstrap health probe.
    let health = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200, "/api/health should be 200");

    // Exercises the executor tool-probe + the agents route.
    let agents = client
        .get(format!("{base}/api/agents"))
        .send()
        .await
        .unwrap();
    assert_eq!(agents.status(), 200, "/api/agents should be 200");
    let agents_json: Value = agents.json().await.unwrap();
    assert!(
        agents_json.is_array(),
        "/api/agents should return a JSON array"
    );

    // Exercises the refactored sessions route + the store read path. Loopback is
    // no-auth, so no bearer token is needed.
    let sessions = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(sessions.status(), 200, "/api/sessions should be 200");
    let _sessions_json: Value = sessions.json().await.unwrap();

    println!(
        "health=200 agents={} sessions=200 — server boots clean",
        agents_json.as_array().map(|a| a.len()).unwrap_or(0)
    );
}
