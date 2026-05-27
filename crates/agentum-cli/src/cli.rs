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
                  agentum serve          # resumes sessions + starts dashboard"
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

        /// Tool binary to run inside the session (claude, codex, cursor, agent, opencode, aider, terminal…). Required.
        ///
        /// Use `terminal` (or `bash`) for a plain interactive shell session.
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

        /// Skip permission prompts for the underlying agent. The flag's
        /// spelling differs per tool (claude: --dangerously-skip-permissions,
        /// codex: --dangerously-bypass-approvals-and-sandbox, cursor/agent:
        /// --force, gemini: --yolo) — the executor adapter picks the right
        /// one. Silently ignored for tools without a known YOLO flag.
        #[arg(long)]
        yolo: bool,
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

        /// Detach from the terminal and run as a background daemon.
        /// The process survives terminal close; logs go to
        /// $XDG_STATE_HOME/agentum/daemon.log (or ~/.local/state/agentum/daemon.log).
        #[arg(long, short = 'd')]
        detach: bool,

        /// Disable authentication entirely. All API routes are accessible
        /// without a bearer token. The dashboard will not show a login screen.
        /// Use only on localhost or a fully trusted private network.
        #[arg(long)]
        no_auth: bool,
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
    ///
    /// Aliased as `tui` for back-compat. The standalone `lazyagentum` binary
    /// drops you straight into the same UI.
    #[command(alias = "tui")]
    Terminal {
        /// Override API base URL (defaults to https://127.0.0.1:8822 → http fallback).
        /// To connect to a remote agentum, e.g. `--api https://my-vps:8822`.
        /// Wins over `--profile` when both are given.
        #[arg(long)]
        api: Option<String>,

        /// Pre-pin the server's SHA-256 cert fingerprint (e.g. `AB:CD:…` as
        /// printed by `agentum serve` on the host). Skips the interactive
        /// trust prompt on first contact.
        #[arg(long)]
        fingerprint: Option<String>,

        /// Skip TLS certificate verification entirely. Strongly discouraged;
        /// here only for local throwaway test setups.
        #[arg(long)]
        insecure: bool,

        /// Mute system sounds for notifications. Also honoured via the
        /// `AGENTUM_TUI_NO_SOUND` env var.
        #[arg(long)]
        no_sound: bool,

        /// Named endpoint profile to load. Manage profiles with
        /// `agentum profiles list/add/remove/use`. Falls back to the
        /// file's `default` entry, then the loopback probe.
        #[arg(long)]
        profile: Option<String>,
    },

    /// Manage the SSH-style known_hosts file used by `agentum terminal`.
    Hosts {
        #[command(subcommand)]
        action: HostsCmd,
    },

    /// Manage named connection profiles (multiple agentum endpoints).
    ///
    /// A *profile* is a named (URL, optional fingerprint) pair so you
    /// can switch between several agentum servers without retyping
    /// `--api …` every time. The TUI's Ctrl-S overlay and the
    /// dashboard's endpoint chip drive the same store.
    ///
    /// Examples:
    ///   agentum profiles add vps https://my-vps.example.com:8822 \
    ///                          --fingerprint AB:CD:... --set-default
    ///   agentum profiles list
    ///   agentum terminal --profile vps
    ///   agentum profiles use vps
    #[command(after_help = "\
EXAMPLES:
    Add a remote endpoint and set it as default:
        agentum profiles add vps https://my-vps:8822 --set-default

    Add another endpoint without making it default:
        agentum profiles add staging https://staging:8822 --fingerprint AB:CD

    Connect to a specific profile:
        agentum terminal --profile staging

    List all profiles (the default is marked with `*`):
        agentum profiles list
")]
    Profiles {
        #[command(subcommand)]
        action: ProfilesCmd,
    },

    /// Manage the board (planner agent output surface).
    ///
    /// These subcommands are how the planner agent creates goals and cards.
    /// The bearer token is read from `credentials.toml` — never from argv or
    /// env vars.
    Board {
        #[command(subcommand)]
        cmd: BoardCmd,
    },

    /// Remove the agentum binary and everything it wrote to disk.
    ///
    /// Wipes the database, TLS material, daemon logs, and the binary
    /// itself. By default keeps the user's profiles + credentials so
    /// a reinstall lands you back at your remote servers; pass
    /// `--all` to remove those too. On Linux also stops and disables
    /// the systemd user unit if the installer registered one.
    ///
    /// Use `--dry-run` to preview, `--yes` to skip the confirmation
    /// prompt (handy in scripts).
    Uninstall {
        /// Skip the y/N confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Also remove user data (profiles, credentials, pinned hosts).
        #[arg(long)]
        all: bool,
        /// Print what would be removed without removing anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Run the local clipboard agent — long-poll the daemon for
    /// clipboard requests and upload PNG bytes of the local clipboard
    /// image on demand. Defaults to running the loop forever against
    /// every profile in `profiles.toml`. The action flags
    /// (`--install`, `--uninstall`, `--status`, `--logs`) are mutually
    /// exclusive.
    ClipAgent {
        /// Only attach to this profile (default: every profile).
        #[arg(long)]
        profile: Option<String>,
        /// Register the launchd plist (macOS) or systemd user unit
        /// (Linux) so the agent starts at login. Idempotent.
        #[arg(long, conflicts_with_all = ["uninstall", "status", "logs"])]
        install: bool,
        /// Remove the launchd plist or systemd unit. Idempotent.
        #[arg(long, conflicts_with_all = ["install", "status", "logs"])]
        uninstall: bool,
        /// Print JSON `{loaded, active, connected_profiles, log_path}`.
        #[arg(long, conflicts_with_all = ["install", "uninstall", "logs"])]
        status: bool,
        /// Print the last 100 lines of the clip-agent log file.
        #[arg(long, conflicts_with_all = ["install", "uninstall", "status"])]
        logs: bool,
    },

    /// Update agentum to the latest release (re-runs install.sh).
    ///
    /// Downloads `releases/latest/download/install.sh` and pipes it to `sh`,
    /// preserving your `INSTALL_DIR`. Pass `--mode server|cli` to skip the
    /// interactive prompt; otherwise the installer behaves identically to a
    /// fresh `curl … | sh` (interactive when on a TTY, defaults to `server`
    /// when not). Pass `--skip-clip-agent` to skip the post-install
    /// `clip-agent --install` invocation (useful in CI / non-tty scripts).
    Update {
        /// Install mode for the installer (`server` = full Control Plane,
        /// `cli` = lightweight CLI, `both` = show both post-install guides).
        /// Omit to let the installer prompt or pick its default.
        #[arg(long, value_parser = ["server", "cli", "both"])]
        mode: Option<String>,

        /// Reinstall even when already on the latest version.
        #[arg(long)]
        force: bool,

        /// Skip the post-install `clip-agent --install` invocation.
        /// Sets AGENTUM_INSTALL_NO_CLIP_AGENT=1 in the spawned sh env
        /// so the installer's autostart hook becomes a no-op. Useful
        /// in CI or scripted updates where you don't want to register
        /// a launchd plist / systemd user unit.
        #[arg(long)]
        skip_clip_agent: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum BoardCmd {
    /// Create a new goal card on the board.
    ///
    /// Prints the new AG-key to stdout (one line). The planner agent parses
    /// this from its pane scrollback to chain subsequent `add-card` calls.
    AddGoal {
        /// Goal title.
        #[arg(long)]
        title: String,
        /// Optional Markdown body for the goal card.
        #[arg(long)]
        body: Option<String>,
        /// Optional working directory hint stored on the goal.
        #[arg(long)]
        workdir: Option<String>,
        /// Named connection profile to use. Defaults to `local`.
        #[arg(long, default_value = "local")]
        profile: String,
    },
    /// Create an execution card under an existing goal.
    ///
    /// Prints the new AG-key to stdout. `--blocks` accepts a comma-separated
    /// list of symbolic keys this card must finish before its dependents can
    /// start. Unknown keys produce exit 5 so the planner can retry after
    /// creating the missing target.
    AddCard {
        /// AG-key of the parent goal (e.g. `AG-7K9X`).
        #[arg(long)]
        parent_goal: String,
        /// Card title.
        #[arg(long)]
        title: String,
        /// Optional Markdown body. The symbolic `--key` is prepended
        /// automatically as `key: <k>\n\n<body>`.
        #[arg(long)]
        body: Option<String>,
        /// Symbolic key for this card (`[a-zA-Z0-9_-]{1,64}`).
        #[arg(long)]
        key: String,
        /// Comma-separated list of symbolic keys this card blocks.
        #[arg(long)]
        blocks: Option<String>,
        /// Label (e.g. `feat`, `fix`, `chore`).
        #[arg(long)]
        lbl: Option<String>,
        /// Named connection profile to use. Defaults to `local`.
        #[arg(long, default_value = "local")]
        profile: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostsCmd {
    /// List pinned hosts and their fingerprints.
    List,
    /// Forget a pinned host (also drops its cached login token).
    Forget { host: String },
}

#[derive(Debug, Subcommand)]
pub enum ProfilesCmd {
    /// List configured profiles. The default (if any) is marked.
    List,
    /// Create or update a profile.
    Add {
        /// Profile name (alphanumeric, `.`, `_`, `-`).
        name: String,
        /// Base URL, e.g. `https://my-vps.example.com:8822`.
        url: String,
        /// Pre-pinned SHA-256 fingerprint (`AB:CD:…`).
        #[arg(long)]
        fingerprint: Option<String>,
        /// Skip TLS verification when connecting to this profile.
        #[arg(long)]
        insecure: bool,
        /// Make this the default profile (used when `--profile` is
        /// omitted on `agentum terminal`).
        #[arg(long)]
        set_default: bool,
    },
    /// Delete a profile. Does not touch its cached credentials —
    /// run `agentum hosts forget HOST:PORT` if you also want to drop
    /// the bearer token.
    Rm { name: String },
    /// Set or clear the default profile. Pass `--clear` to remove.
    Use {
        /// Profile name to mark as default. Required unless `--clear`.
        name: Option<String>,
        /// Clear the default profile pointer.
        #[arg(long)]
        clear: bool,
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
    /// Run the first-time setup wizard to create the admin account.
    ///
    /// Works without a running server — writes directly to the database.
    /// Pass --username and --password together for non-interactive (script/CI) use.
    Setup {
        /// Admin username. Prompted interactively if omitted.
        #[arg(long)]
        username: Option<String>,
        /// Admin password. Prompted interactively if omitted.
        #[arg(long)]
        password: Option<String>,
    },
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

/// Args struct the dispatcher hands to `commands::clip_agent::run`. Mirrors
/// the `Cmd::ClipAgent` variant's fields one-for-one. Defined as a struct
/// (rather than passing the enum variant directly) so the subcommand
/// module can take a single value, and so tests can construct
/// representative arg sets without going through clap.
#[derive(Debug, Default, Clone)]
pub struct ClipAgentArgs {
    pub profile: Option<String>,
    pub install: bool,
    pub uninstall: bool,
    pub status: bool,
    pub logs: bool,
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
            yolo,
        } => crate::commands::new::run(name, tool, dir, pick, model, arg, up, yolo).await,
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
            detach,
            no_auth,
        } => {
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let cert_addr: SocketAddr = format!("{host}:{cert_port}").parse()?;
            crate::commands::serve::run(addr, cert_addr, !no_tls, no_resume, detach, no_auth).await
        }
        Cmd::Auth { action } => crate::commands::auth::run(action).await,
        Cmd::Config { action } => crate::commands::config::run(action).await,
        Cmd::Doctor => crate::commands::doctor::run().await,
        Cmd::Terminal {
            api,
            fingerprint,
            insecure,
            no_sound,
            profile,
        } => {
            crate::commands::terminal::run(crate::commands::terminal::Options {
                api,
                fingerprint,
                insecure,
                no_sound,
                profile,
            })
            .await
        }
        Cmd::Hosts { action } => crate::commands::hosts::run(action).await,
        Cmd::Profiles { action } => crate::commands::profiles::run(action).await,
        Cmd::Board { cmd } => crate::commands::board::run(cmd).await,
        Cmd::Uninstall { yes, all, dry_run } => {
            crate::commands::uninstall::run(crate::commands::uninstall::Options {
                yes,
                all,
                dry_run,
            })
            .await
        }
        Cmd::ClipAgent {
            profile,
            install,
            uninstall,
            status,
            logs,
        } => {
            crate::commands::clip_agent::run(ClipAgentArgs {
                profile,
                install,
                uninstall,
                status,
                logs,
            })
            .await
        }
        Cmd::Update {
            mode,
            force,
            skip_clip_agent,
        } => {
            let mode = match mode.as_deref() {
                Some("server") => Some(crate::commands::update::Mode::Server),
                Some("cli") => Some(crate::commands::update::Mode::Cli),
                Some("both") => Some(crate::commands::update::Mode::Both),
                _ => None,
            };
            crate::commands::update::run(mode, force, skip_clip_agent).await
        }
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
    fn terminal_parses() {
        use clap::Parser;
        let cli = Cli::parse_from(["agentum", "terminal"]);
        assert!(matches!(cli.command, Cmd::Terminal { api: None, .. }));

        let cli = Cli::parse_from(["agentum", "terminal", "--api", "http://1.2.3.4:9000"]);
        match cli.command {
            Cmd::Terminal { api, .. } => {
                assert_eq!(api.as_deref(), Some("http://1.2.3.4:9000"));
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn terminal_accepts_fingerprint_and_insecure() {
        use clap::Parser;
        let cli = Cli::parse_from([
            "agentum",
            "terminal",
            "--api",
            "https://vps:8822",
            "--fingerprint",
            "AB:CD",
            "--insecure",
            "--no-sound",
        ]);
        match cli.command {
            Cmd::Terminal {
                api,
                fingerprint,
                insecure,
                no_sound,
                profile,
            } => {
                assert_eq!(api.as_deref(), Some("https://vps:8822"));
                assert_eq!(fingerprint.as_deref(), Some("AB:CD"));
                assert!(insecure);
                assert!(no_sound);
                assert!(profile.is_none());
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn terminal_accepts_profile() {
        use clap::Parser;
        let cli = Cli::parse_from(["agentum", "terminal", "--profile", "vps"]);
        match cli.command {
            Cmd::Terminal { profile, .. } => {
                assert_eq!(profile.as_deref(), Some("vps"));
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn tui_alias_still_works() {
        use clap::Parser;
        let cli = Cli::parse_from(["agentum", "tui"]);
        assert!(matches!(cli.command, Cmd::Terminal { api: None, .. }));
    }
}
