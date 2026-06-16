//! `lazyagentum` — short-circuit binary that drops you straight into the
//! agentum terminal dashboard, the same way `lazygit` lands you in the
//! lazygit UI without subcommands. Equivalent to `agentum terminal` but
//! with no other commands attached.

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "lazyagentum",
    version,
    about = "Drop into the agentum terminal dashboard with one keystroke.",
    long_about = "Equivalent to `agentum terminal`. Boots `agentum-server` \
                  in-process and opens the dashboard — no separate daemon to \
                  start. Use `--api`/`--profile` to target a remote machine \
                  instead."
)]
struct Args {
    /// Override the API base URL, targeting a remote server instead of the
    /// in-process embedded one. When omitted, the TUI runs its own server.
    #[arg(long)]
    api: Option<String>,

    /// Pre-pin the server's SHA-256 cert fingerprint, skipping the prompt.
    #[arg(long)]
    fingerprint: Option<String>,

    /// Skip TLS cert verification entirely. Strongly discouraged.
    #[arg(long)]
    insecure: bool,

    /// Mute system sounds for notifications. Also honoured via the
    /// `AGENTUM_TUI_NO_SOUND` env var.
    #[arg(long)]
    no_sound: bool,

    /// Named endpoint profile to load (manage with
    /// `agentum profiles add NAME …`). Falls back to the file's
    /// `default = …` when omitted.
    #[arg(long)]
    profile: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Tracing must go to a file, never stderr — we own the alt-screen.
    agentum::init_tracing_for_tui();
    let args = Args::parse();
    agentum::commands::terminal::run(agentum::commands::terminal::Options {
        api: args.api,
        fingerprint: args.fingerprint,
        insecure: args.insecure,
        no_sound: args.no_sound,
        profile: args.profile,
    })
    .await
}
