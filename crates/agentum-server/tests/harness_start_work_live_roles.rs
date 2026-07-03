//! LIVE companion to `harness_start_work_live.rs` with the SDD roles loop ON
//! (spec 006 D1 default): the FIRST spawn is the PM role gate, not the feature
//! agent (spec 008 architecture §1). Proves the PM-gate-first path also spawns a
//! real session and lands the spec-grounded prompt.
//!
//! `#[ignore]` — real `claude` + tmux; a separate binary from the roles-off test
//! so each owns its `std::process::exit(0)`. Run it explicitly:
//!
//!   cargo test -p agentum-server --test harness_start_work_live_roles -- --ignored --nocapture

#[path = "support_start_work/mod.rs"]
mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real claude agent (PM role gate first); run with --ignored"]
async fn start_work_route_with_sdd_roles_spawns_the_pm_gate() {
    support::run(true).await;
    std::process::exit(0);
}
