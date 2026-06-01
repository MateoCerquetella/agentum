//! `agentum terminal` — interactive terminal dashboard (also reachable as
//! the `lazyagentum` shim binary, and aliased as `agentum tui`).
//!
//! Thin client. Talks to a running `agentum serve` over the same HTTP/WS
//! API the Svelte SPA uses. It never opens the database or touches tmux
//! directly. The lazygit side pane spawns a *local* PTY independently.
//!
//! Connection trust: SSH-style. The first time we hit an `https://` host
//! we don't already have pinned, we display its SHA-256 fingerprint and
//! ask the operator to confirm it matches what `agentum serve` printed
//! on the host TTY. Once accepted, the pin is persisted to
//! `$XDG_CONFIG_HOME/agentum/known_hosts.toml`. Mismatch on subsequent
//! connect aborts with a loud "MITM?" error.

mod api;
mod app;
mod extensions;
mod iometer;
mod palette;
mod prefs;
pub mod profiles;
mod pty;
mod sound;
mod term;
mod theme;
pub mod trust;
mod ui;

// Re-export YOLO constants so the standalone `agentum new` command can
// stay aligned with the TUI's New Session form without making the entire
// `app` module public.
pub use app::{YOLO_FLAG, YOLO_TOOLS};

use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use url::Url;

use api::TlsTrust;

const DEFAULT_HTTPS: &str = "https://127.0.0.1:8822";
const DEFAULT_HTTP: &str = "http://127.0.0.1:8822";

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub api: Option<String>,
    /// Pre-supply a SHA-256 fingerprint to pin without prompting. Useful
    /// when the user copied it from the host TTY into a script.
    pub fingerprint: Option<String>,
    /// Skip cert verification entirely. Strongly discouraged; only here
    /// for local throwaway test setups. Print a big warning when set.
    pub insecure: bool,
    /// Mute system sounds for notifications. Also honoured via the
    /// `AGENTUM_TUI_NO_SOUND` env var so users can set it once in their
    /// shell rc instead of remembering the flag.
    pub no_sound: bool,
    /// Named server profile to load (`agentum profiles add NAME …`).
    /// Resolved before `--api`: an explicit `--api` always wins so a
    /// profile-using user can still override the URL ad-hoc. When
    /// neither is set, `profiles.toml`'s `default = …` is consulted
    /// before the loopback probe.
    pub profile: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    // Connect-or-onboard loop: if the requested daemon (profile, --api,
    // or loopback) doesn't answer, an interactive TTY user gets a
    // numbered menu to add a remote server and retry. Non-TTY
    // invocations (CI, scripts) fall through to the same bail! they
    // got before — the prompt would just hang.
    let initial_opts = opts.clone();
    let mut current_opts = apply_profile(opts).await?;
    let (client, base, sessions) = loop {
        match connect_once(&current_opts).await {
            Ok(connected) => break connected,
            Err(e) => {
                if !is_interactive_tty() {
                    return Err(e);
                }
                match prompt_unreachable_menu(&current_opts, &e)? {
                    // The only daemon is the local one; there's no "add a
                    // remote server" path anymore (remote machines are SSH
                    // hosts). Any non-quit choice just retries the local
                    // daemon — handy right after `agentum serve`.
                    UnreachableAction::Quit => return Err(e),
                    _ => {
                        current_opts = apply_profile(initial_opts.clone()).await?;
                    }
                }
            }
        }
    };
    let _ = base; // base is bundled in `client`; kept for future use.

    let mut client = client;
    let mut sessions = sessions;
    let mut pending_after: Option<app::PendingAfterSwitch> = None;
    loop {
        let sound_muted =
            current_opts.no_sound || std::env::var_os("AGENTUM_TUI_NO_SOUND").is_some();
        // One daemon, so every session's ops (start/stop/stream) go
        // through the single local client: the ops map is empty and
        // `client_for_session` defaults every id to the "" (local) key.
        // The sidebar tree groups by *host* instead — that key is derived
        // from each session's own `host_label` inside `Tree::build`.
        let session_profile: HashMap<uuid::Uuid, String> = HashMap::new();
        let outcome = run_tui_session(
            client,
            sessions,
            sound_muted,
            None,
            pending_after.take(),
            Vec::new(),
            session_profile,
        )
        .await?;
        match outcome {
            app::RunOutcome::Quit => return Ok(()),
            app::RunOutcome::SwitchProfile { then, .. } => {
                // Hosts don't switch the daemon (there's only one). Treat
                // a lingering switch/reconnect request as a reconnect to
                // the local daemon — useful right after it restarts.
                current_opts = apply_profile(initial_opts.clone()).await?;
                let connected = connect_once(&current_opts).await?;
                client = connected.0;
                sessions = connected.2;
                pending_after = then;
            }
        }
    }
}

/// One full TUI lifetime: enter alt-screen, run the event loop, tear
/// down. Returning a `RunOutcome` lets `run` decide whether to quit or
/// reconnect with a new profile.
async fn run_tui_session(
    client: api::Client,
    sessions: Vec<agentum_core::Session>,
    sound_muted: bool,
    active_profile: Option<String>,
    pending: Option<app::PendingAfterSwitch>,
    extras: Vec<(String, ProfileConnect)>,
    session_profile: HashMap<uuid::Uuid, String>,
) -> Result<app::RunOutcome> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    // Mouse capture lets us see scroll-wheel events instead of the host
    // terminal's fallback of translating them to arrow keys (which then
    // get forwarded into claude code). Mirrors Alacritty / kitty / iTerm
    // when the running app doesn't have its own mouse tracking on. We
    // *don't* forward the events to the inner pane — they drive
    // agentum's own scrollback. Side effect: native click-drag selection
    // breaks; users hold Shift to bypass app-mode capture, which is the
    // standard convention every modern terminal emulator honours.
    execute!(stdout, EnableMouseCapture).context("enable mouse capture")?;
    // Bracketed paste collapses a multi-line paste into a single
    // `CtEvent::Paste(String)` instead of N synthetic key events. Without
    // it, a long paste flooded `handle_key`, triggered N ratatui redraws
    // (one per char), and locked the UI long enough that Ctrl-Q couldn't
    // get a slot to abort. Best-effort: terminals that don't understand
    // `\x1b[?2004h` ignore it and we fall back to key-by-key paste.
    let _ = execute!(stdout, EnableBracketedPaste);
    let _restore = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("init ratatui terminal")?;

    app::run_loop(
        &mut terminal,
        client,
        sessions,
        sound_muted,
        active_profile,
        pending,
        extras,
        session_profile,
    )
    .await
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Reverse order of setup: disable mouse capture first (otherwise
        // the host terminal stays in app-mode tracking after agentum
        // exits and ordinary clicks emit garbage in the parent shell).
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = execute!(io::stdout(), DisableMouseCapture);
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Decide what TLS-trust posture to use for `base`. Plain HTTP is trivial.
/// HTTPS goes through known_hosts pinning, with explicit `--insecure` and
/// `--fingerprint` overrides taking precedence over the file.
async fn establish_trust(base: &Url, opts: &Options) -> Result<TlsTrust> {
    if base.scheme() == "http" {
        return Ok(TlsTrust::Plain);
    }
    if opts.insecure {
        // --insecure disables cert verification entirely, so confine it to
        // the local machine. On a remote host an on-path attacker could MITM
        // the connection; require an explicit pinned --fingerprint there.
        if !is_local_loopback(base) {
            bail!(
                "--insecure refused for non-loopback host {}: TLS verification can only be \
                 disabled for the local machine (127.0.0.1/localhost/::1). For a remote daemon, \
                 pin its certificate with --fingerprint <sha256> instead.",
                base.host_str().unwrap_or("?")
            );
        }
        eprintln!(
            "{}: TLS cert verification disabled (--insecure). The connection is NOT \
             protected against MITM attackers on the network path. Use only for local \
             testing.",
            warn_label()
        );
        return Ok(TlsTrust::AcceptAny);
    }

    let host_key = trust::host_key(base)?;
    let mut known = trust::KnownHosts::load()?;

    // Explicit --fingerprint always wins. We persist it for next time
    // (so the user only types it once).
    if let Some(raw) = &opts.fingerprint {
        let fp = trust::normalize_fingerprint(raw)
            .with_context(|| format!("invalid --fingerprint: {raw}"))?;
        known.add(host_key.clone(), fp.clone())?;
        return Ok(TlsTrust::Pinned(fp));
    }

    if let Some(pinned) = known.pin(&host_key) {
        return Ok(TlsTrust::Pinned(pinned.to_string()));
    }

    // First contact: fetch what the server is presenting and ask the user
    // to verify it matches what the host's TTY printed.
    eprintln!("First contact with {host_key}.");
    eprintln!("Fetching the server's TLS certificate so you can verify it…");
    let actual = trust::fetch_fingerprint(base)
        .await
        .with_context(|| format!("could not reach {base} for fingerprint check"))?;
    eprintln!();
    eprintln!("  Server fingerprint (SHA-256):");
    eprintln!("    {actual}");
    eprintln!();
    eprintln!("Confirm this matches the line `agentum serve` printed on the host:");
    eprintln!("    TLS cert fingerprint (verify on second device): SHA-256 …");
    eprintln!();
    eprint!("Trust this fingerprint and pin it for future connections? [y/N] ");
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read trust prompt")?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!("aborted — refusing to connect to an untrusted host");
    }
    known.add(host_key.clone(), actual.clone())?;
    eprintln!("Pinned. Subsequent connects will verify this fingerprint silently.");
    Ok(TlsTrust::Pinned(actual))
}

fn warn_label() -> &'static str {
    // ANSI yellow if stderr is a tty, otherwise plain.
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        "\x1b[1;33mWARNING\x1b[0m"
    } else {
        "WARNING"
    }
}

/// Get a usable bearer token: try the per-host cached one first, validate
/// it, else prompt for username/password and exchange via /api/auth/login.
///
/// On failure (bad credentials, network error, etc.) the user gets up to
/// three attempts before we surface the error. Password reads use
/// `rpassword` so the typed characters don't echo — pre-v0.7.9 this used a
/// plain `stdin().read_line` and the password rendered in the terminal,
/// which on macOS surfaced as "I typed credentials and nothing happened"
/// because the user couldn't tell which characters were the password vs.
/// echo noise.
async fn obtain_token(base: &Url, trust_setting: &TlsTrust) -> Result<String> {
    // Fast-path: server has auth disabled (--no-auth). Return a dummy token;
    // the middleware accepts every request regardless of token value.
    if let Ok(true) = api::auth_is_disabled(base, trust_setting).await {
        return Ok("no-auth".to_string());
    }

    let host_key = trust::host_key(base)?;
    let mut creds = trust::Credentials::load()?;

    if let Some(cached) = creds.token(&host_key) {
        if probe_token(base, trust_setting, cached).await {
            return Ok(cached.to_string());
        }
        // Cached token rejected (rotated, expired, server reset). Drop it.
        let _ = creds.remove(&host_key);
    }

    // First-run shortcut for the local loopback: when the daemon is
    // fresh (zero users registered) and we're the same user who could
    // log in interactively anyway, auto-create a `local` account with
    // a random password and cache the resulting token. This is what
    // turns the auto-spawned sidecar into a zero-prompt experience —
    // the user just runs `agentum terminal` and lands in the TUI
    // without ever seeing a login screen. We only do this for the
    // loopback host because anonymous register on a remote daemon
    // would be a security footgun.
    if is_local_loopback(base)
        && let Ok(true) = api::auth_needs_setup(base, trust_setting).await
    {
        let password = generate_random_password();
        match api::register(base, trust_setting, "local", &password).await {
            Ok(token) => {
                creds.put(host_key.clone(), token.clone(), Some("local".to_string()))?;
                return Ok(token);
            }
            Err(e) => {
                // If register raced with another caller (unlikely on
                // loopback, but possible if two TUIs start in
                // parallel), fall through to the login prompt below.
                tracing::debug!(error = %e, "loopback auto-register failed; falling back to login prompt");
            }
        }
    }

    use std::io::{self, Write};
    eprintln!();
    eprintln!("Sign in to agentum at {base}");

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            eprintln!();
            eprintln!("  attempt {attempt} of {MAX_ATTEMPTS} — try again");
        }
        eprint!("  username: ");
        io::stderr().flush().ok();
        let mut user = String::new();
        io::stdin().read_line(&mut user).context("read username")?;
        let user = user.trim().to_string();

        // rpassword reads from /dev/tty when available, falling back to
        // stdin. The "  password: " prompt is written to stderr by us so
        // it lines up visually with the username field. `prompt_password`
        // would write its own to stdout; we drive it manually.
        eprint!("  password: ");
        io::stderr().flush().ok();
        let pw = match rpassword::read_password() {
            Ok(p) => p,
            Err(e) => {
                return Err(anyhow!("read password: {e}"));
            }
        };

        eprintln!("  signing in…");
        match api::login(base, trust_setting, &user, &pw).await {
            Ok(token) => {
                creds.put(host_key, token.clone(), Some(user))?;
                eprintln!("  ✓ signed in");
                return Ok(token);
            }
            Err(e) => {
                eprintln!("  ✗ {e}");
                last_err = Some(e);
                // Continue the loop and re-prompt unless we're out of attempts.
            }
        }
    }

    Err(last_err
        .map(|e| anyhow!("login failed after {MAX_ATTEMPTS} attempts: {e}"))
        .unwrap_or_else(|| anyhow!("login failed after {MAX_ATTEMPTS} attempts")))
}

/// `true` when `base` points at 127.0.0.1 or `localhost` — used by
/// the auto-bootstrap path to decide whether anonymous registration
/// is safe. Anonymous register on a remote daemon would be a
/// security footgun (anyone reachable creates an admin), so we gate
/// the convenience strictly to the local machine.
fn is_local_loopback(base: &Url) -> bool {
    matches!(
        base.host_str(),
        Some("127.0.0.1") | Some("localhost") | Some("::1")
    )
}

/// Cryptographically random password for the bootstrap `local` user.
/// 32 chars from a URL-safe alphabet — plenty of entropy, easy to
/// stash in credentials.toml. The user never types this; it's
/// generated, written to the loopback daemon's user DB, and the
/// resulting bearer token is what's cached. Re-runs of
/// `agentum terminal` use the cached token, not this password.
fn generate_random_password() -> String {
    // This password is a real credential — it's registered as the
    // loopback `local` user's password — so it must come from the OS
    // CSPRNG, not a time/pid-seeded PRNG (which a local attacker could
    // predict and use to register/log in before us). The 64-char
    // alphabet divides 256 evenly, so `byte % 64` is unbiased.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG unavailable");
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

async fn probe_token(base: &Url, trust_setting: &TlsTrust, token: &str) -> bool {
    let Ok(client) = api::Client::new(base.clone(), token.to_string(), trust_setting.clone())
    else {
        return false;
    };
    client.me().await.is_ok()
}

/// Result of attempting a non-interactive connect to one named
/// profile. Used by the multi-profile fanout in [`run`] so the
/// sidebar can render every server with a coherent status (live,
/// unreachable, login-needed) instead of blocking on prompts the
/// user may not want to fill for every server at startup.
pub struct ProfileConnect {
    pub status: app::ServerStatus,
    pub client: Option<api::Client>,
    pub sessions: Vec<agentum_core::Session>,
    pub last_error: Option<String>,
    pub agent_availability: Option<std::collections::HashSet<String>>,
    /// Daemon version reported by `/api/health` (e.g. `"0.7.61"`).
    /// `None` for unreachable/never-probed/older daemons that didn't
    /// expose the field. The sidebar uses this to surface fleet
    /// version drift so the user can spot peers behind the local CLI.
    pub version: Option<String>,
}

/// Layer profile defaults under any explicit CLI flags. Returns the
/// merged `Options` so the rest of `run()` reads from one source. A
/// missing profile name resolves to the file's `default = …`; an
/// explicit `--profile` that doesn't exist is an error so the user
/// notices typos instead of silently dropping back to the loopback.
/// Auto-start a local `agentum serve` sidecar in the background when
/// no daemon is listening on the loopback. The user shouldn't have to
/// remember to run `agentum serve` in another window before
/// `agentum terminal` — on the host they're already at, the TUI
/// should just work. The sidecar lives in its own process group and
/// outlives the TUI on purpose so subsequent `agentum terminal` runs
/// reuse the same daemon (and its SQLite user DB / token storage).
///
/// Returns `true` once the new daemon answers a health probe, `false`
/// if the spawn failed or it didn't come up within the budget. The
/// caller falls back to its prior behaviour on `false` (configured
/// remote default, or the connect-or-onboard menu).
async fn try_autostart_local_daemon() -> bool {
    // Only attempt this when we're the user-launched binary — if
    // `current_exe` is unresolvable (uncommon edge case) we can't
    // safely re-spawn ourselves.
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // Spawn `<exe> serve` in a detached child. `setsid`/process-group
    // detach happens implicitly via `Command::spawn` without inheriting
    // the parent's controlling terminal — the child writes its logs to
    // a temp file rather than the parent's stdout/stderr so the alt-
    // screen UI never sees daemon output.
    let log_dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))
        })
        .map(|p| p.join("agentum"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("autoserve.log");
    let log_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    let log_err = match log_file.try_clone() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(log_file)
        .stderr(log_err);
    // Detach from the parent's process group on Unix so a TUI exit
    // doesn't take the sidecar with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // setsid() to start a new session — frees the child
                // from the parent's controlling terminal.
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let Ok(_child) = cmd.spawn() else {
        return false;
    };
    // Wait up to ~3 s for the daemon to bind + answer. The handshake
    // is fast on a warm cache; the loop exits the moment it answers.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if loopback_alive().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// Quick non-interactive health probe of the local loopback daemon
/// across HTTPS-then-HTTP. Used by `apply_profile` to decide whether
/// to prefer the local daemon over a configured `default = …` when
/// the user didn't pass `--profile`. Bounded to a short timeout so
/// the boot path doesn't pay the full TCP handshake latency on a
/// host that isn't running `agentum serve`.
async fn loopback_alive() -> bool {
    // HTTP-first: --no-tls is the common local-dev setup.
    for candidate in [DEFAULT_HTTP, DEFAULT_HTTPS] {
        let Ok(base) = Url::parse(candidate) else {
            continue;
        };
        let trust = if base.scheme() == "https" {
            let host_key = trust::host_key(&base).unwrap_or_default();
            trust::KnownHosts::load()
                .ok()
                .and_then(|kh| kh.pin(&host_key).map(|s| s.to_string()))
                .map(TlsTrust::Pinned)
                .unwrap_or(TlsTrust::AcceptAny)
        } else {
            TlsTrust::Plain
        };
        if api::probe_health(&base, &trust, Duration::from_millis(500))
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn apply_profile(mut opts: Options) -> Result<Options> {
    let profiles = match profiles::load() {
        Ok(p) => p,
        Err(e) => {
            // A broken / missing profiles file should never block the
            // common path of running with no profiles at all. Surface
            // the parse error to the user, then carry on with whatever
            // they passed on the command line.
            eprintln!(
                "{}: failed to load profiles.toml: {e}\n  Continuing without profile defaults.",
                warn_label()
            );
            return Ok(opts);
        }
    };

    // Resolution rules when no `--profile` flag is present:
    //   1. If the local loopback daemon answers, prefer it. Launching
    //      `agentum terminal` on a host with a live local daemon
    //      should drive that host — `default = "omarchy"` in
    //      profiles.toml is a *fallback*, not an override, because the
    //      most common reason to run the TUI is to drive the machine
    //      you're sitting at. The user can still target the configured
    //      default explicitly via `agentum terminal --profile <name>`.
    //   2. Otherwise, fall back to the configured `default = …`.
    //   3. Otherwise, fall through to the connect-or-onboard loop
    //      (which itself does loopback discovery + the unreachable
    //      menu).
    //
    // Explicit `--profile` always wins. The probe is bounded to
    // 500 ms so the boot path stays snappy on offline machines.
    let active = match opts.profile.clone() {
        Some(name) => Some(name),
        None => {
            // Prefer the local loopback whenever it answers. If it
            // doesn't, try to auto-spawn `agentum serve` in the
            // background — running the TUI on a machine should not
            // require remembering to start a daemon by hand. The
            // sidecar lives in its own process group so it outlives
            // the TUI and gets reused on subsequent launches.
            if loopback_alive().await || try_autostart_local_daemon().await {
                None
            } else {
                profiles.default_name().map(str::to_string)
            }
        }
    };
    let Some(name) = active else {
        return Ok(opts);
    };

    let Some(profile) = profiles.get(&name) else {
        // Explicit `--profile` to a missing entry is a hard error;
        // a stale `default` field merits the same loud failure so
        // typos get caught.
        bail!(
            "profile `{name}` not found in {} — list with `agentum profiles list`",
            profiles.path().display()
        );
    };

    if opts.api.is_none() {
        opts.api = Some(profile.url.clone());
    }
    if opts.fingerprint.is_none() {
        opts.fingerprint = profile.fingerprint.clone();
    }
    // The profile's `insecure` is opt-in; we OR it with the CLI flag
    // so an --insecure on the command line is never quietly discarded.
    opts.insecure = opts.insecure || profile.insecure;
    // Write the resolved profile name back to `opts.profile` so the
    // rest of the TUI knows *which named profile* this connection
    // belongs to — not just "we connected to some URL". This matters
    // for the default-profile path (`agentum terminal` with no
    // `--profile` flag but a `default = "omarchy"` in profiles.toml):
    // without this writeback, `current_opts.profile` stays `None`,
    // `active_key` ends up `""` in the session-list-tagging in
    // `mod.rs` (the `merge_sessions_dedup` call site), and every
    // session from the remote daemon gets tagged as loopback. The
    // sidebar then files them under the hostname-derived local row
    // ("macbook-pro") instead of `@omarchy`, which is exactly the
    // "why are my omarchy sessions on my macbook?" bug the user hit.
    opts.profile = Some(name);
    Ok(opts)
}

async fn resolve_base(override_url: Option<String>) -> Result<Url> {
    if let Some(s) = override_url {
        let parsed = Url::parse(&s).with_context(|| format!("invalid --api URL: {s}"))?;
        // For an explicit HTTPS URL, probe it first. If the health probe
        // fails (e.g. the server is running with --no-tls and speaks plain
        // HTTP), automatically retry with the HTTP equivalent so users
        // don't have to manually fix their URL or profile.
        if parsed.scheme() == "https" {
            let host_key = trust::host_key(&parsed).unwrap_or_default();
            let probe_trust = trust::KnownHosts::load()
                .ok()
                .and_then(|kh| kh.pin(&host_key).map(|s| s.to_string()))
                .map(TlsTrust::Pinned)
                .unwrap_or(TlsTrust::AcceptAny);
            if api::probe_health(&parsed, &probe_trust, Duration::from_millis(750))
                .await
                .is_err()
            {
                // HTTPS probe failed — try plain HTTP for --no-tls setups.
                let mut http = parsed.clone();
                let _ = http.set_scheme("http");
                if api::probe_health(&http, &TlsTrust::Plain, Duration::from_millis(750))
                    .await
                    .is_ok()
                {
                    return Ok(http);
                }
            }
        }
        return Ok(parsed);
    }
    // Loopback discovery: HTTP-first since --no-tls is common for local dev.
    for candidate in [DEFAULT_HTTP, DEFAULT_HTTPS] {
        let parsed = Url::parse(candidate).expect("static URL");
        let trust = if parsed.scheme() == "https" {
            // Loopback HTTPS means the user's own self-signed cert. Any
            // pinned record in known_hosts? Use it; otherwise accept-any
            // for the loopback probe (we're talking to our own machine).
            let host_key = trust::host_key(&parsed).unwrap_or_default();
            trust::KnownHosts::load()
                .ok()
                .and_then(|kh| kh.pin(&host_key).map(|s| s.to_string()))
                .map(TlsTrust::Pinned)
                .unwrap_or(TlsTrust::AcceptAny)
        } else {
            TlsTrust::Plain
        };
        if api::probe_health(&parsed, &trust, Duration::from_millis(750))
            .await
            .is_ok()
        {
            return Ok(parsed);
        }
    }
    Err(anyhow!(
        "no agentum daemon found on local machine ({DEFAULT_HTTP} or {DEFAULT_HTTPS}). \
         Start one with `agentum serve` or connect to a remote server. \
         Run `agentum profiles add <name> <url>` to save a remote endpoint, \
         then `agentum terminal --profile <name>`."
    ))
}

// ---------- empty-daemon onboarding ----------

/// One full attempt to materialise an authenticated [`api::Client`] for
/// `opts`. Returns the client, the resolved base URL, and an initial
/// session list. Used by [`run`]'s connect-or-onboard loop so a probe
/// failure can ask the user "want to add an server?" before bailing.
async fn connect_once(opts: &Options) -> Result<(api::Client, Url, Vec<agentum_core::Session>)> {
    let base = resolve_base(opts.api.clone()).await?;
    let trust = establish_trust(&base, opts).await?;
    let token = obtain_token(&base, &trust).await?;
    let client = api::Client::new(base.clone(), token, trust)?;
    client.health().await.with_context(|| {
        format!("agentum daemon not reachable at {base} — start it with `agentum serve`")
    })?;
    let sessions = client
        .list_sessions()
        .await
        .context("failed to list sessions")?;
    Ok((client, base, sessions))
}

#[derive(Debug)]
enum UnreachableAction {
    AddServer,
    Retry,
    Quit,
}

/// Stdin and stdout both have to be on a TTY for the prompt to make
/// sense — `agentum terminal | tee log` would otherwise hang waiting
/// on input that never comes. Plain non-TTY callers fall back to the
/// pre-existing bail behaviour.
fn is_interactive_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Print a numbered menu and read a single keystroke + Enter from
/// stdin. Loops on invalid input. The caller is expected to be on
/// the controlling terminal — see `is_interactive_tty`.
fn prompt_unreachable_menu(opts: &Options, err: &anyhow::Error) -> Result<UnreachableAction> {
    let (target, hint) = if let Some(ref api) = opts.api {
        (
            api.clone(),
            "Check the URL is correct and the daemon is running on that host.",
        )
    } else {
        (
            format!("local daemon ({DEFAULT_HTTPS})"),
            "No remote server is configured. Start `agentum serve` locally, or add a remote endpoint.",
        )
    };
    eprintln!();
    eprintln!("  agentum couldn't reach a daemon at {target}.");
    eprintln!("  ↳ {err}");
    eprintln!();
    eprintln!("  {hint}");
    eprintln!();
    eprintln!("  What would you like to do?");
    eprintln!("    [1] Add a remote server");
    eprintln!("    [2] Retry (e.g. after running `agentum serve` in another window)");
    eprintln!("    [3] Quit");
    eprintln!();
    loop {
        eprint!("  > ");
        io::stderr().flush().ok();
        let mut buf = String::new();
        let n = io::stdin().lock().read_line(&mut buf)?;
        if n == 0 {
            // EOF — treat like Quit so a piped/closed stdin doesn't loop.
            return Ok(UnreachableAction::Quit);
        }
        match buf.trim() {
            "1" | "a" | "add" => return Ok(UnreachableAction::AddServer),
            "2" | "r" | "retry" => return Ok(UnreachableAction::Retry),
            "3" | "q" | "quit" | "exit" | "" => return Ok(UnreachableAction::Quit),
            other => {
                eprintln!("  unrecognised input: {other:?} — pick 1, 2, or 3");
                continue;
            }
        }
    }
}
