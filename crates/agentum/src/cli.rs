use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agentum",
    version,
    about = "Self-hosted control plane for AI coding agents.",
    long_about = "Self-hosted control plane for AI coding agents.\n\n\
                  Quick start:\n  \
                  agentum new my-session --tool claude --dir . --up\n  \
                  agentum serve",
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Create a new agent session.
    New {
        /// Session name (used in tmux target and URLs).
        name: String,

        /// Tool binary to run inside the session (claude, codex, opencode, aider…). Required.
        #[arg(long)]
        tool: String,

        /// Working directory the agent starts in. Required.
        #[arg(long)]
        dir: PathBuf,

        /// Optional model identifier passed through to the tool.
        #[arg(long)]
        model: Option<String>,

        /// Repeatable: `--arg key=value` becomes `--key=value` on the tool's command line.
        /// Use `--arg key=true` for boolean flags (forwarded as `--key`).
        #[arg(long = "arg", value_name = "KEY=VAL")]
        arg: Vec<String>,

        /// Start the session immediately after creating it.
        #[arg(long)]
        up: bool,
    },

    /// Start a session.
    Up {
        /// Session name.
        name: String,
    },

    /// Stop a session gracefully.
    Down {
        /// Session name.
        name: String,
    },

    /// Kill a session immediately.
    Kill {
        /// Session name.
        name: String,
    },

    /// List sessions.
    Ls {
        /// Show only running sessions.
        #[arg(long)]
        running: bool,
    },

    /// Start the dashboard server.
    Serve {
        /// Port to bind on (HTTPS by default).
        #[arg(long, default_value_t = 8822)]
        port: u16,

        /// Bind address.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Plain-HTTP cert-server port (PRD §3) — serves the self-signed PEM
        /// for trust-on-first-use from a phone.
        #[arg(long, default_value_t = 8823)]
        cert_port: u16,

        /// Bind plain HTTP instead of HTTPS. Disables the cert-server too.
        #[arg(long)]
        no_tls: bool,
    },

    /// Show running sessions.
    Ps,

    /// Manage API authentication.
    Auth {
        #[command(subcommand)]
        action: AuthCmd,
    },

    /// Check system health (tmux, dirs, db, certs, port).
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Print the current bearer token (creates one if missing).
    Show,
    /// Generate a fresh bearer token and overwrite the on-disk file.
    Rotate,
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::New {
            name,
            tool,
            dir,
            model,
            arg,
            up,
        } => crate::commands::new::run(name, tool, dir, model, arg, up).await,
        Cmd::Up { name } => crate::commands::up::run(name).await,
        Cmd::Down { name } => crate::commands::down::run(name).await,
        Cmd::Kill { name } => crate::commands::kill::run(name).await,
        Cmd::Ls { running } => crate::commands::ls::run(running).await,
        Cmd::Ps => crate::commands::ls::run(true).await,
        Cmd::Serve {
            port,
            host,
            cert_port,
            no_tls,
        } => {
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let cert_addr: SocketAddr = format!("{host}:{cert_port}").parse()?;
            crate::commands::serve::run(addr, cert_addr, !no_tls).await
        }
        Cmd::Auth { action } => crate::commands::auth::run(action).await,
        Cmd::Doctor => crate::commands::doctor::run().await,
    }
}

/// Convert `--arg key=value` entries into `--key=value` shell flags.
/// `key=true` becomes a bare `--key` switch.
pub fn arg_to_flag(raw: &str) -> String {
    match raw.split_once('=') {
        Some((k, "true")) => format!("--{k}"),
        Some((k, v)) => format!("--{k}={v}"),
        None => format!("--{raw}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_translation() {
        assert_eq!(arg_to_flag("model=opus"), "--model=opus");
        assert_eq!(
            arg_to_flag("dangerously-skip-permissions=true"),
            "--dangerously-skip-permissions"
        );
        assert_eq!(arg_to_flag("verbose"), "--verbose");
    }
}
