pub mod accounts;
pub mod agent_status;
pub mod app;
pub mod browser;
pub mod browser_native;
pub mod cache;
pub mod claude_usage;
pub mod cli;
pub mod clipboard;
pub mod codex_usage;
pub mod crash_reports;
pub mod diagnostics;
pub mod feedback;
pub mod fs;
pub mod gh;
pub mod gh_projects;
pub mod github_labels;
pub mod gl;
pub mod hooks;
pub mod hosted_review;
pub mod html_export;
pub mod keybindings;
pub mod linear;
pub mod notebook;
pub mod notifications;
pub mod onboarding;
pub mod open_code_usage;
pub mod permissions;
pub mod pet;
pub mod platform;
pub mod project_groups;
pub mod pty;
pub mod rate_limits;
pub mod remote_workspace;
pub mod repos;
pub mod runtime;
pub mod server;
pub mod session;
pub mod settings;
pub mod shell;
pub mod shell_runtimes;
pub mod skills;
pub mod sparse_presets;
pub mod speech;
pub mod ssh;
pub mod star_nag;
pub mod timestamps;
pub mod ui;
pub mod updater;
pub mod usage_prefs;
pub mod window;
pub mod workspace_cleanup;
pub mod workspace_ports;

// Small single-command namespaces ported as their own modules.
pub mod e2e;
pub mod mobile;
pub mod stats;
pub mod telemetry;
pub mod workspace_space;

// Serializes the unit tests that mutate the process-global `AGENTUM_HOME`.
// Rust runs a crate's tests in parallel threads inside one binary, so two
// tests that each `std::env::set_var("AGENTUM_HOME", …)` race: one can read
// the other's dir mid-run — or a `TempDir` the other already dropped and
// deleted — and writes under it fail. Every such test locks this first; the
// guard must stay alive for the whole test body. `unwrap_or_else` ignores
// poisoning so a failure in one test doesn't cascade into the others.
#[cfg(test)]
pub(crate) static ENV_HOME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
