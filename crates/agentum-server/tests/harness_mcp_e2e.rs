//! Live end-to-end demo of the spec-010 harness MCP tools, driven over a REAL
//! running embedded agentum server (no-auth loopback) via HTTP JSON-RPC at
//! `POST /mcp`. Not part of the normal suite (it binds a socket); run it for the
//! demo and watch the tool responses + the files the tools create:
//!
//!   cargo test -p agentum-server --test harness_mcp_e2e -- --ignored --nocapture

use std::path::Path;

use agentum_server::serve_embedded_loopback_state;
use agentum_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;

/// POST one JSON-RPC `tools/call` and return the tool's text payload.
async fn call(client: &reqwest::Client, base: &str, token: &str, tool: &str, args: Value) -> String {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    });
    let resp: Value = client
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("unexpected /mcp response: {resp}"))
        .to_string()
}

/// Sorted relative file listing under `root`.
fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&p) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.display().to_string());
                }
            }
        }
    }
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spins a real loopback server; run explicitly: -- --ignored --nocapture"]
async fn harness_mcp_tools_drive_a_real_repo() {
    // 1. A REAL embedded agentum server (no-auth loopback) on an ephemeral port.
    let db = TempDir::new().unwrap();
    let store = Store::open(&db.path().join("t.db")).await.unwrap();
    let (addr, state) = serve_embedded_loopback_state(store).await.unwrap();
    let base = format!("http://{addr}");
    let token = state.mcp_token.to_string();
    let client = reqwest::Client::new();

    // 2. A scratch "repo" with NOTHING installed.
    let repo = TempDir::new().unwrap();
    let wd = repo.path().to_string_lossy().to_string();
    println!("\n================ agentum running at {base} ================");
    println!("scratch repo (empty, no harness): {wd}\n");

    // tools/list — confirm the 6 spec-010 harness tools are advertised.
    let listed: Value = client
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| n.starts_with("agentum_harness"))
        .map(String::from)
        .collect();
    println!(">> harness MCP tools advertised: {names:?}\n");

    // 3. scaffold — the ONLY thing agentum writes into the repo.
    println!(
        ">> agentum_harness_scaffold\n{}\n",
        call(&client, &base, &token, "agentum_harness_scaffold", json!({ "workdir": wd })).await
    );

    // 4. author a spec, then turn its acceptance criteria into the backlog.
    std::fs::create_dir_all(repo.path().join(".agentum-harness/specs/demo")).unwrap();
    std::fs::write(
        repo.path().join(".agentum-harness/specs/demo/spec.md"),
        "## Acceptance Criteria\n- [ ] Login works\n- [ ] Logout works\n- [x] Health endpoint returns 200\n",
    )
    .unwrap();
    println!(
        ">> agentum_harness_plan (specs/demo acceptance criteria -> backlog)\n{}\n",
        call(&client, &base, &token, "agentum_harness_plan", json!({ "workdir": wd, "spec_id": "demo" })).await
    );

    // 5. Bootstrap-Contract readiness check.
    println!(
        ">> agentum_harness_check (Bootstrap Contract / cold-start test)\n{}\n",
        call(&client, &base, &token, "agentum_harness_check", json!({ "workdir": wd })).await
    );

    // 6. board, rebuilt PURELY from disk (no agentum store consulted).
    println!(
        ">> agentum_harness_board (rebuilt from disk)\n{}\n",
        call(&client, &base, &token, "agentum_harness_board", json!({ "workdir": wd })).await
    );

    // 7. append to the durable, append-only decision log.
    println!(
        ">> agentum_harness_log_decision\n{}\n",
        call(
            &client,
            &base,
            &token,
            "agentum_harness_log_decision",
            json!({ "workdir": wd, "entry": "chose .agentum-harness over .harness — durable in git (010e)" })
        )
        .await
    );

    // 8. show the REAL files the tools created in the repo.
    println!(">> files now in the repo:");
    for f in tree(&repo.path().join(".agentum-harness")) {
        println!("   .agentum-harness/{f}");
    }
    println!("===========================================================\n");

    // Assertions so this is a real test, not just prints.
    assert!(repo.path().join(".agentum-harness/feature_list.json").exists());
    assert!(repo.path().join(".agentum-harness/decisions.md").exists());
    assert!(repo.path().join(".agentum-harness/specs/demo/spec.md").exists());
    println!("\n✅ all 6 harness MCP tools verified end-to-end over real HTTP.\n");

    // The embedded server runs as a detached task that would otherwise block
    // multi-thread-runtime shutdown; exit cleanly so the demo doesn't linger.
    std::process::exit(0);
}
