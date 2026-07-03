//! LIVE: the start-work ROUTE drives a real agent from an issue (roles OFF).
//!
//! `#[ignore]` — spawns a real `claude` CLI in a real tmux pane, so it never
//! runs in CI (and NOT in the `--lib` gate). It is the merge gate for spec 008
//! F1's sacred readiness-bool change (D5): a HUMAN runs it green pre-release,
//! like the staging browser-QA step. Run it explicitly:
//!
//!   cargo test -p agentum-server --test harness_start_work_live -- --ignored --nocapture
//!
//! Covers the leg `harness_live_agent.rs` skips: issue → `POST
//! /api/harness/start-work` → a session opens → the prompt lands → the issue's
//! `status/*` labels flip. See `support_start_work/mod.rs` for the driver.

#[path = "support_start_work/mod.rs"]
mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real claude agent; run with --ignored"]
async fn start_work_route_drives_a_real_agent_from_an_issue() {
    // Roles OFF → the first spawn is the FEATURE agent (deterministic).
    support::run(false).await;
    // The embedded server's background workers keep the runtime busy, so a
    // natural return would hang on teardown — exit cleanly once assertions pass
    // (same reason as `harness_live_agent.rs`).
    std::process::exit(0);
}
