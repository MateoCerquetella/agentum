use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agentum",
    version,
    about = "Control plane for AI coding agents.",
    long_about = "Control plane for AI coding agents.\n\n\
                  Quick start:\n  \
                  agentum new my-session --tool claude --dir .\n  \
                  agentum terminal       # open the interactive dashboard"
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

    /// Remove dead sessions the control plane still tracks (zombies left by
    /// crashed agents). Dry-run by default — pass --yes to remove. Never touches
    /// running/idle sessions, and only acts on sessions agentum manages (tmux
    /// sessions started outside agentum are never in the store, so they're safe).
    Prune {
        /// Actually remove the sessions (default prints a dry-run preview).
        #[arg(long, short = 'y')]
        yes: bool,

        /// Also prune `stopped` sessions, not just `crashed` ones.
        #[arg(long)]
        stopped: bool,
    },

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

    /// One-glance summary of the control plane this CLI is pointed at
    /// (sessions, worktrees, hosts). Reaches the desktop's embedded server when
    /// run inside an agentum pane (`$AGENTUM_API_URL`), else the configured
    /// profile or `127.0.0.1:8822`.
    Status {
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },

    /// Inspect the worktrees the control plane knows about. Reaches the same
    /// server as `status` (the desktop's embedded server inside a pane).
    Worktree {
        #[command(subcommand)]
        action: WorktreeCmd,
    },

    /// Inter-agent orchestration: mail (send/check/reply/inbox), a task DAG
    /// (task-create/list/update), and dispatch. Backed by /api/orchestration.
    Orchestration {
        #[command(subcommand)]
        action: OrchestrationCmd,
    },

    /// List the desktop's open browser tabs (label + url). Requires the desktop
    /// app (the standalone daemon has no webviews).
    Tab {
        #[command(subcommand)]
        action: TabCmd,
    },

    /// Read what's available from the active browser tab. Requires the desktop.
    Snapshot {
        #[arg(long)]
        tab: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Navigate the active browser tab to a URL. Requires the desktop.
    Navigate {
        url: String,
        #[arg(long)]
        tab: Option<String>,
    },

    /// Click an element in the active browser tab by CSS selector. Requires the desktop.
    Click {
        #[arg(long)]
        selector: String,
        #[arg(long)]
        tab: Option<String>,
    },

    /// Fill an input in the active browser tab by CSS selector. Requires the desktop.
    Fill {
        #[arg(long)]
        selector: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        tab: Option<String>,
    },

    /// macOS computer-use: inspect and drive local desktop apps via the
    /// Accessibility tree. Requires the desktop app on macOS.
    Computer {
        #[command(subcommand)]
        action: ComputerCmd,
    },

    /// Run a shell command in a terminal session and capture its output.
    /// Best-effort: sends the command + a done-marker and reads the pane back.
    Exec {
        /// Target session (name or id prefix). See `agentum terminal list`.
        #[arg(long)]
        session: String,
        /// The shell command to run.
        #[arg(long)]
        command: String,
        /// Seconds to wait for the command to finish.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Launch the interactive terminal dashboard — OR, with a subcommand
    /// (`list`/`read`/`send`/`wait`), drive agentum-managed terminal sessions
    /// over the API. Bare `agentum terminal` still opens the TUI (back-compat).
    ///
    /// Aliased as `tui` for back-compat. The standalone `lazyagentum` binary
    /// drops you straight into the same UI.
    #[command(alias = "tui")]
    Terminal {
        /// Terminal-control verb. Omitted → launch the interactive TUI.
        #[command(subcommand)]
        action: Option<TerminalCmd>,

        /// Override API base URL (defaults to https://127.0.0.1:8822 → http fallback).
        /// To connect to a remote agentum, e.g. `--api https://my-vps:8822`.
        /// Wins over `--profile` when both are given.
        #[arg(long)]
        api: Option<String>,

        /// Pre-pin the server's SHA-256 cert fingerprint (e.g. `AB:CD:…` as
        /// printed by the remote server on the host). Skips the interactive
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
    /// preserving your `INSTALL_DIR`. There is only one install — this
    /// machine runs the daemon — so the installer behaves identically to a
    /// fresh `curl … | sh` (interactive when on a TTY, non-interactive when
    /// not). Pass `--skip-clip-agent` to skip the post-install
    /// `clip-agent --install` invocation (useful in CI / non-tty scripts).
    Update {
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
pub enum TabCmd {
    /// List open browser tabs.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Open a new browser tab navigated to URL, printing the new tab id.
    Open {
        url: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ComputerCmd {
    /// Report which computer-use ops are available + whether AX is granted.
    Capabilities,
    /// Report the Accessibility permission status.
    Permissions,
    /// List on-screen apps (name + pid).
    ListApps {
        #[arg(long)]
        json: bool,
    },
    /// Dump an app's Accessibility element tree (role/title/value by index).
    GetAppState {
        #[arg(long)]
        app: String,
        #[arg(long)]
        json: bool,
    },
    /// Press the element at the given index (AXPress).
    Click {
        #[arg(long)]
        app: String,
        #[arg(long = "element-index")]
        element_index: usize,
    },
    /// Set an element's value (AXValue).
    SetValue {
        #[arg(long)]
        app: String,
        #[arg(long = "element-index")]
        element_index: usize,
        #[arg(long)]
        value: String,
    },
    /// Type text into an app.
    TypeText {
        #[arg(long)]
        app: String,
        #[arg(long)]
        text: String,
    },
    /// Press a named key (Return/Tab/Escape/arrows/…) in an app.
    PressKey {
        #[arg(long)]
        app: String,
        #[arg(long)]
        key: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrchestrationCmd {
    /// Send a message to a handle or group (`@all`/`@idle`/`@claude`/…).
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long = "type")]
        msg_type: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        thread_id: Option<String>,
        /// JSON payload string.
        #[arg(long)]
        payload: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read this terminal's NEW (unread) messages and consume them. Use `inbox`
    /// for a non-consuming view of the whole mailbox.
    Check {
        /// Recipient handle; defaults to $AGENTUM_TERMINAL_HANDLE.
        #[arg(long)]
        terminal: Option<String>,
        /// Filter by message type(s), comma-separated.
        #[arg(long, value_delimiter = ',')]
        types: Vec<String>,
        /// Leave messages unread (peek instead of consume).
        #[arg(long)]
        no_mark_read: bool,
        /// Block until a matching message arrives or the timeout elapses.
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 120_000)]
        timeout_ms: u64,
        #[arg(long)]
        json: bool,
    },
    /// Reply to a message by id (goes back to its sender on the same thread).
    Reply {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        body: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List this terminal's mailbox without consuming it.
    Inbox {
        #[arg(long)]
        terminal: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        json: bool,
    },
    /// Create a task (optionally depending on other task ids).
    TaskCreate {
        #[arg(long)]
        spec: String,
        /// JSON array of dependency task ids, e.g. `[1,2]`.
        #[arg(long)]
        deps: Option<String>,
        #[arg(long)]
        parent: Option<i64>,
        #[arg(long)]
        json: bool,
    },
    /// List tasks, optionally by status or only `--ready` ones.
    TaskList {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        json: bool,
    },
    /// Update a task's status (DAG dependents auto-promote on `completed`).
    TaskUpdate {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        status: String,
        /// JSON result string.
        #[arg(long)]
        result: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Assign a task to a handle; `--inject` also sends the spec to its pane.
    Dispatch {
        #[arg(long)]
        task: i64,
        #[arg(long)]
        to: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        inject: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show a task and its dispatch contexts.
    DispatchShow {
        #[arg(long)]
        task: i64,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TerminalCmd {
    /// List terminal sessions (name, status, tool).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print the last N lines of a session's pane.
    Read {
        /// Session name or id prefix.
        name: String,
        #[arg(long, default_value_t = 40)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
    /// Send text to a session (Enter appended unless `--no-enter`).
    Send {
        /// Session name or id prefix.
        name: String,
        /// Text to send (joined with spaces).
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long)]
        no_enter: bool,
    },
    /// Block until a session's pane contains TEXT, or `--timeout` elapses.
    Wait {
        /// Session name or id prefix.
        name: String,
        #[arg(long)]
        text: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorktreeCmd {
    /// List all known worktrees (name + branch).
    List {
        /// Emit the raw worktree JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show the worktree the current directory (or `$AGENTUM_WORKTREE_PATH`)
    /// belongs to. Exits with the worktree's name, or a "not inside" notice.
    Current {
        #[arg(long)]
        json: bool,
    },
    /// Remove stale worktrees sessions left behind. Dry-run by default — pass
    /// --yes to remove. Prunes git-prunable (gone) worktrees; add --clean to
    /// also remove non-primary worktrees with no uncommitted changes. NEVER
    /// removes the primary worktree, locked worktrees, or any tree with
    /// uncommitted work. Host-aware (covers remote SSH repos too).
    Prune {
        /// Limit to one repo (its id); omit to sweep every registered repo.
        #[arg(long)]
        repo: Option<String>,

        /// Also remove clean (no-uncommitted-changes) non-primary worktrees,
        /// not just git-prunable ones.
        #[arg(long)]
        clean: bool,

        /// Actually remove the worktrees (default prints a dry-run preview).
        #[arg(long, short = 'y')]
        yes: bool,

        /// Emit the raw prune JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostsCmd {
    /// List SSH-agentless hosts controlled by the local daemon.
    List,
    /// Add an SSH host, then set it up in one flow: check what's there,
    /// install the required deps (tmux, git), and ask which agent CLIs to
    /// install. `--yes` installs everything missing without prompting.
    Add {
        name: String,
        #[arg(long)]
        user: String,
        #[arg(long)]
        hostname: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(long)]
        key: Option<String>,
        /// Install all missing deps + agents without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Re-run the setup flow on an existing host: check, install required
    /// deps, ask which agents to install. `--yes` installs all missing.
    Setup {
        name: String,
        /// Install all missing deps + agents without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Probe an SSH host by name (one-line ready/not-ready summary).
    Test { name: String },
    /// Full readiness report for a host: required deps (tmux, git), agent
    /// CLIs, detected package manager, and install hints. Exits non-zero
    /// when a required dependency is missing.
    Readiness { name: String },
    /// Remove an SSH host by name. Refuses when sessions still reference it.
    Rm { name: String },
    /// Forget a pinned Agentum server certificate host (legacy trust store).
    Forget { host: String },
    /// Kill zombie tmux sessions on a host — orphaned `agentum-*` panes a
    /// crashed/abandoned session left running. Dry-run by default; pass --yes
    /// to kill. NEVER touches attached sessions, sessions backed by a live
    /// (running/idle) record, externally-attached sessions, or any tmux a user
    /// started outside agentum.
    PruneTmux {
        name: String,
        /// Actually kill the zombie sessions (default prints a dry-run preview).
        #[arg(long, short = 'y')]
        yes: bool,
    },
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
        Cmd::Prune { yes, stopped } => crate::commands::prune::run(yes, stopped).await,
        Cmd::Open { name } => crate::commands::open::run(name).await,
        Cmd::Tail {
            name,
            lines,
            follow,
        } => crate::commands::tail::run(name, lines, follow).await,
        Cmd::Send { name, text } => crate::commands::send::run(name, text).await,
        Cmd::Keys { name, key_spec } => crate::commands::keys::run(name, key_spec).await,
        Cmd::Auth { action } => crate::commands::auth::run(action).await,
        Cmd::Config { action } => crate::commands::config::run(action).await,
        Cmd::Doctor => crate::commands::doctor::run().await,
        Cmd::Status { json } => crate::commands::status::run(json).await,
        Cmd::Worktree { action } => match action {
            WorktreeCmd::List { json } => crate::commands::worktree::list(json).await,
            WorktreeCmd::Current { json } => crate::commands::worktree::current(json).await,
            WorktreeCmd::Prune {
                repo,
                clean,
                yes,
                json,
            } => crate::commands::worktree::prune(repo, clean, yes, json).await,
        },
        Cmd::Terminal {
            action,
            api,
            fingerprint,
            insecure,
            no_sound,
            profile,
        } => match action {
            // A control verb drives a session over the API; no subcommand opens
            // the TUI (the original behaviour, preserved for back-compat).
            Some(TerminalCmd::List { json }) => crate::commands::terminal_control::list(json).await,
            Some(TerminalCmd::Read { name, lines, json }) => {
                crate::commands::terminal_control::read(name, lines, json).await
            }
            Some(TerminalCmd::Send {
                name,
                text,
                no_enter,
            }) => crate::commands::terminal_control::send(name, text, no_enter).await,
            Some(TerminalCmd::Wait {
                name,
                text,
                timeout,
            }) => crate::commands::terminal_control::wait(name, text, timeout).await,
            None => {
                crate::commands::terminal::run(crate::commands::terminal::Options {
                    api,
                    fingerprint,
                    insecure,
                    no_sound,
                    profile,
                })
                .await
            }
        },
        Cmd::Orchestration { action } => dispatch_orchestration(action).await,
        Cmd::Tab { action } => match action {
            TabCmd::List { json } => crate::commands::browser::tab_list(json).await,
            TabCmd::Open { url, json } => crate::commands::browser::tab_open(url, json).await,
        },
        Cmd::Snapshot { tab, json } => crate::commands::browser::snapshot(tab, json).await,
        Cmd::Navigate { url, tab } => crate::commands::browser::navigate(url, tab).await,
        Cmd::Click { selector, tab } => crate::commands::browser::click(selector, tab).await,
        Cmd::Fill {
            selector,
            text,
            tab,
        } => crate::commands::browser::fill(selector, text, tab).await,
        Cmd::Computer { action } => dispatch_computer(action).await,
        Cmd::Exec {
            session,
            command,
            timeout,
        } => crate::commands::terminal_control::exec(session, command, timeout).await,
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
            force,
            skip_clip_agent,
        } => crate::commands::update::run(force, skip_clip_agent).await,
    }
}

/// Convert `--arg key=value` entries into `--key=value` shell flags.
/// Route an `orchestration` subcommand to its handler. Kept separate from the
/// main `dispatch` match so that large variant set stays readable.
async fn dispatch_orchestration(action: OrchestrationCmd) -> Result<()> {
    use crate::commands::orchestration as orch;
    match action {
        OrchestrationCmd::Send {
            to,
            subject,
            from,
            body,
            msg_type,
            priority,
            thread_id,
            payload,
            json,
        } => {
            orch::send(
                to, subject, from, body, msg_type, priority, thread_id, payload, json,
            )
            .await
        }
        OrchestrationCmd::Check {
            terminal,
            types,
            no_mark_read,
            wait,
            timeout_ms,
            json,
        } => orch::check(terminal, types, no_mark_read, wait, timeout_ms, json).await,
        OrchestrationCmd::Reply {
            id,
            body,
            from,
            json,
        } => orch::reply(id, body, from, json).await,
        OrchestrationCmd::Inbox {
            terminal,
            limit,
            json,
        } => orch::inbox(terminal, limit, json).await,
        OrchestrationCmd::TaskCreate {
            spec,
            deps,
            parent,
            json,
        } => orch::task_create(spec, deps, parent, json).await,
        OrchestrationCmd::TaskList {
            status,
            ready,
            json,
        } => orch::task_list(status, ready, json).await,
        OrchestrationCmd::TaskUpdate {
            id,
            status,
            result,
            json,
        } => orch::task_update(id, status, result, json).await,
        OrchestrationCmd::Dispatch {
            task,
            to,
            from,
            inject,
            json,
        } => orch::dispatch(task, to, from, inject, json).await,
        OrchestrationCmd::DispatchShow { task, json } => orch::dispatch_show(task, json).await,
    }
}

/// Route a `computer` subcommand to its handler.
async fn dispatch_computer(action: ComputerCmd) -> Result<()> {
    use crate::commands::computer as cu;
    match action {
        ComputerCmd::Capabilities => cu::capabilities().await,
        ComputerCmd::Permissions => cu::permissions().await,
        ComputerCmd::ListApps { json } => cu::list_apps(json).await,
        ComputerCmd::GetAppState { app, json } => cu::get_app_state(app, json).await,
        ComputerCmd::Click { app, element_index } => cu::click(app, element_index).await,
        ComputerCmd::SetValue {
            app,
            element_index,
            value,
        } => cu::set_value(app, element_index, value).await,
        ComputerCmd::TypeText { app, text } => cu::type_text(app, text).await,
        ComputerCmd::PressKey { app, key } => cu::press_key(app, key).await,
    }
}

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
                action,
                api,
                fingerprint,
                insecure,
                no_sound,
                profile,
            } => {
                assert!(action.is_none(), "no subcommand → TUI launch");
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
