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
                  agentum new my-session --tool claude --dir .\n  \
                  agentum serve          # resumes sessions + starts dashboard",
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

        /// Working directory the agent starts in. Defaults to the current
        /// directory. Mutually exclusive with `--pick`.
        #[arg(long, conflicts_with = "pick")]
        dir: Option<PathBuf>,

        /// Interactively pick the workdir with `lf` (terminal file manager).
        /// `lf` must be installed on `PATH`.
        #[arg(long, short = 'P')]
        pick: bool,

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

    /// Remove a session (must be stopped unless --force).
    Rm {
        /// Session name.
        name: String,

        /// Kill the session first if it is still running.
        #[arg(long)]
        force: bool,
    },

    /// List sessions.
    Ls {
        /// Show only running sessions.
        #[arg(long)]
        running: bool,

        /// Filter by tool name.
        #[arg(long)]
        tool: Option<String>,
    },

    /// Show running sessions.
    Ps,

    /// Attach to a session's tmux pane (detach: Ctrl-b d).
    Open {
        /// Session name.
        name: String,
    },

    /// Show pane log output.
    Tail {
        /// Session name.
        name: String,

        /// Number of lines to show.
        #[arg(short = 'n', default_value_t = 30)]
        lines: u32,

        /// Follow output as it grows.
        #[arg(short = 'f', long)]
        follow: bool,
    },

    /// Send text to a session (appends Enter).
    Send {
        /// Session name.
        name: String,

        /// Text to send.
        text: String,
    },

    /// Send raw tmux key sequence to a session (e.g. 'C-c', 'Enter').
    Keys {
        /// Session name.
        name: String,

        /// tmux key specification.
        key_spec: String,
    },

    /// Start agentum (resumes sessions + launches dashboard).
    Serve {
        /// Port to bind on (HTTPS by default).
        #[arg(long, default_value_t = 8822)]
        port: u16,

        /// Bind address.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Plain-HTTP cert-server port — serves the self-signed PEM
        /// for trust-on-first-use from a phone.
        #[arg(long, default_value_t = 8823)]
        cert_port: u16,

        /// Bind plain HTTP instead of HTTPS. Disables the cert-server too.
        #[arg(long)]
        no_tls: bool,

        /// Skip auto-resuming stopped sessions on startup.
        #[arg(long)]
        no_resume: bool,
    },

    /// Manage API authentication.
    Auth {
        #[command(subcommand)]
        action: AuthCmd,
    },

    /// Manage configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },

    /// Check system health (tmux, dirs, db, certs, port).
    Doctor,

    /// Launch the interactive terminal dashboard.
    Tui {
        /// Override API base URL (defaults to https://127.0.0.1:8822 → http fallback).
        #[arg(long)]
        api: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// List registered users.
    List,
    /// Add a user. Prompts for password unless --password is given.
    Add {
        username: String,
        /// Set the password non-interactively (e.g. for scripts).
        #[arg(long)]
        password: Option<String>,
    },
    /// Delete a user (and all their sessions).
    Rm { username: String },
    /// Wipe ALL users + sessions. Next register on the dashboard re-bootstraps.
    Reset,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print a configuration value.
    Get {
        /// Configuration key.
        key: String,
    },
    /// Set a configuration value.
    Set {
        /// Configuration key.
        key: String,
        /// Value to set.
        value: String,
    },
    /// Open config file in $EDITOR.
    Edit,
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::New {
            name,
            tool,
            dir,
            pick,
            model,
            arg,
            up,
        } => crate::commands::new::run(name, tool, dir, pick, model, arg, up).await,
        Cmd::Up { name } => crate::commands::up::run(name).await,
        Cmd::Down { name } => crate::commands::down::run(name).await,
        Cmd::Kill { name } => crate::commands::kill::run(name).await,
        Cmd::Rm { name, force } => crate::commands::rm::run(name, force).await,
        Cmd::Ls { running, tool } => crate::commands::ls::run(running, tool).await,
        Cmd::Ps => crate::commands::ls::run(true, None).await,
        Cmd::Open { name } => crate::commands::open::run(name).await,
        Cmd::Tail {
            name,
            lines,
            follow,
        } => crate::commands::tail::run(name, lines, follow).await,
        Cmd::Send { name, text } => crate::commands::send::run(name, text).await,
        Cmd::Keys { name, key_spec } => crate::commands::keys::run(name, key_spec).await,
        Cmd::Serve {
            port,
            host,
            cert_port,
            no_tls,
            no_resume,
        } => {
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let cert_addr: SocketAddr = format!("{host}:{cert_port}").parse()?;
            crate::commands::serve::run(addr, cert_addr, !no_tls, no_resume).await
        }
        Cmd::Auth { action } => crate::commands::auth::run(action).await,
        Cmd::Config { action } => crate::commands::config::run(action).await,
        Cmd::Doctor => crate::commands::doctor::run().await,
        Cmd::Tui { api } => crate::commands::tui::run(api).await,
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

    #[test]
    fn tui_parses() {
        use clap::Parser;
        let cli = Cli::parse_from(["agentum", "tui"]);
        assert!(matches!(cli.command, Cmd::Tui { api: None }));

        let cli = Cli::parse_from(["agentum", "tui", "--api", "http://1.2.3.4:9000"]);
        match cli.command {
            Cmd::Tui { api } => assert_eq!(api.as_deref(), Some("http://1.2.3.4:9000")),
            _ => panic!("expected Tui"),
        }
    }
}
