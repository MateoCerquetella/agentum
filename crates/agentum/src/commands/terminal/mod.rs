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
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
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
    let mut current_opts = apply_profile(opts)?;
    let (client, base, sessions) = loop {
        match connect_once(&current_opts).await {
            Ok(connected) => break connected,
            Err(e) => {
                if !is_interactive_tty() {
                    return Err(e);
                }
                match prompt_unreachable_menu(&current_opts, &e)? {
                    UnreachableAction::AddServer => {
                        let new_profile = interactive_add_profile()?;
                        // Re-resolve from a fresh base so the next attempt
                        // honours the just-added profile (and any default
                        // pointer it set). `--api` from the original CLI
                        // call is preserved only when it was the explicit
                        // override path; if the user is here it's because
                        // that override didn't reach a daemon, so we
                        // respect their new profile choice instead.
                        let mut next = initial_opts.clone();
                        next.api = None;
                        next.fingerprint = None;
                        next.profile = Some(new_profile);
                        current_opts = apply_profile(next)?;
                    }
                    UnreachableAction::Retry => {
                        // No-op: just loop. Useful when the user just
                        // ran `agentum serve` in another window.
                    }
                    UnreachableAction::Quit => return Err(e),
                }
            }
        }
    };
    let _ = base; // base is bundled in `client`; kept for future use.

    let mut client = client;
    let mut sessions = sessions;
    let mut active_profile_name = current_opts.profile.clone();
    let mut pending_after: Option<app::PendingAfterSwitch> = None;
    loop {
        let sound_muted =
            current_opts.no_sound || std::env::var_os("AGENTUM_TUI_NO_SOUND").is_some();
        // Fan out to every *other* configured profile in parallel so
        // the sidebar can show a unified, multi-server view of the
        // user's fleet. The default profile (the one we just got a
        // live `client` for above) is already covered; the rest go
        // through `try_connect_profile`, which never prompts.
        let extras = fanout_other_profiles(active_profile_name.as_deref().unwrap_or("")).await;
        // Aggregate sessions: default profile's first, then every
        // reachable peer's. Two profiles pointing at the same daemon
        // share session ids; dedupe by id so the sidebar can't show
        // the same session under two servers. Active profile wins
        // ties, otherwise named beats the loopback "" key.
        let active_key = active_profile_name.clone().unwrap_or_default();
        let mut per_profile: Vec<(String, Vec<agentum_core::Session>)> =
            Vec::with_capacity(1 + extras.len());
        per_profile.push((active_key.clone(), std::mem::take(&mut sessions)));
        for (pname, conn) in &extras {
            per_profile.push((pname.clone(), conn.sessions.clone()));
        }
        let (merged, session_profile) = app::merge_sessions_dedup(per_profile, &active_key);
        sessions = merged;
        let outcome = run_tui_session(
            client,
            sessions,
            sound_muted,
            active_profile_name.clone(),
            pending_after.take(),
            extras,
            session_profile,
        )
        .await?;
        match outcome {
            app::RunOutcome::Quit => return Ok(()),
            app::RunOutcome::SwitchProfile { name, then } => {
                // Re-resolve the entire connection from the chosen
                // profile and re-enter the TUI. Errors here drop back
                // into the same connect-or-onboard loop the cold start
                // uses, so a misconfigured remote server is recoverable
                // without restarting the binary.
                let mut next = initial_opts.clone();
                next.api = None;
                next.fingerprint = None;
                next.profile = Some(name.clone());
                current_opts = apply_profile(next)?;
                let connected = match connect_once(&current_opts).await {
                    Ok(c) => c,
                    Err(e) => {
                        if !is_interactive_tty() {
                            return Err(e);
                        }
                        eprintln!();
                        eprintln!("  ✗ switch to profile `{name}` failed:");
                        eprintln!("    {e}");
                        eprintln!();
                        eprintln!("  staying on the previous server.");
                        // Pause so the user actually sees the error
                        // before the alt-screen redraws over it. Without
                        // this, the failure flashes by in <100ms and the
                        // user thinks "nothing happened" after typing
                        // their credentials.
                        eprint!("  press Enter to continue… ");
                        std::io::stderr().flush().ok();
                        let mut _buf = String::new();
                        let _ = std::io::stdin().read_line(&mut _buf);
                        // Restore the previous profile by re-running the
                        // initial resolution; this re-enters the connect
                        // loop with whatever the user originally asked
                        // for. Anything still failing there will pop the
                        // same prompt as the cold start.
                        current_opts = apply_profile(initial_opts.clone())?;
                        connect_once(&current_opts).await?
                    }
                };
                client = connected.0;
                sessions = connected.2;
                active_profile_name = current_opts.profile.clone();
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
    let host_key = trust::host_key(base)?;
    let mut creds = trust::Credentials::load()?;

    if let Some(cached) = creds.token(&host_key) {
        if probe_token(base, trust_setting, cached).await {
            return Ok(cached.to_string());
        }
        // Cached token rejected (rotated, expired, server reset). Drop it.
        let _ = creds.remove(&host_key);
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

async fn probe_token(base: &Url, trust_setting: &TlsTrust, token: &str) -> bool {
    let Ok(client) = api::Client::new(base.clone(), token.to_string(), trust_setting.clone())
    else {
        return false;
    };
    client.me().await.is_ok()
}

/// Probe every configured profile *except* `skip_name` (the default
/// profile we already connected synchronously in `run`). Returns a
/// list of (profile_name, ProfileConnect) tuples in profile-order.
/// All probes run in parallel via `join_all` so the boot path
/// doesn't serialise on a slow / unresponsive remote server.
async fn fanout_other_profiles(skip_name: &str) -> Vec<(String, ProfileConnect)> {
    let store = match profiles::Profiles::load() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let entries: Vec<(String, profiles::Profile)> = store
        .list()
        .into_iter()
        .filter_map(|(name, profile, _is_default)| {
            if name == skip_name {
                None
            } else {
                Some((name, profile))
            }
        })
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let probes = entries.iter().map(|(_, p)| try_connect_profile(p));
    let results = futures_util::future::join_all(probes).await;
    entries
        .into_iter()
        .zip(results)
        .map(|((name, _), conn)| (name, conn))
        .collect()
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
}

/// Best-effort connect for a single profile. Never prompts — that's
/// the contract that lets the fanout call this for every profile in
/// parallel without freezing the boot path. Failures degrade
/// gracefully into a status the sidebar can render.
async fn try_connect_profile(profile: &profiles::Profile) -> ProfileConnect {
    let unreachable = |msg: String| ProfileConnect {
        status: app::ServerStatus::Unreachable,
        client: None,
        sessions: Vec::new(),
        last_error: Some(msg),
        agent_availability: None,
    };

    let base = match Url::parse(&profile.url) {
        Ok(u) => u,
        Err(e) => return unreachable(format!("invalid URL: {e}")),
    };
    let trust = match trust_for_profile(&base, profile) {
        Ok(t) => t,
        Err(e) => return unreachable(format!("trust: {e}")),
    };
    // Pull a cached token from the per-host credentials cache. No
    // cache → mark login-needed; the user can sign in via the
    // sidebar / overlay later without restarting the TUI.
    let host_key = match trust::host_key(&base) {
        Ok(k) => k,
        Err(e) => return unreachable(format!("host key: {e}")),
    };
    let creds = match trust::Credentials::load() {
        Ok(c) => c,
        Err(e) => return unreachable(format!("creds: {e}")),
    };
    let Some(token) = creds.token(&host_key).map(str::to_string) else {
        return ProfileConnect {
            status: app::ServerStatus::LoginNeeded,
            client: None,
            sessions: Vec::new(),
            last_error: Some(format!("no cached token for {host_key}")),
            agent_availability: None,
        };
    };
    let client = match api::Client::new(base, token, trust) {
        Ok(c) => c,
        Err(e) => return unreachable(format!("build client: {e}")),
    };
    if client.health().await.is_err() {
        return unreachable(format!("health probe failed at {}", profile.url));
    }
    // Health passed but token rejected ⇒ login-needed. We can't
    // prompt here; the user has to log in on this server via the
    // overlay before its sessions appear.
    if client.me().await.is_err() {
        return ProfileConnect {
            status: app::ServerStatus::LoginNeeded,
            client: Some(client),
            sessions: Vec::new(),
            last_error: Some("token rejected".to_string()),
            agent_availability: None,
        };
    }
    let sessions = client.list_sessions().await.unwrap_or_default();
    let agent_availability = match client.list_agents().await {
        Ok(list) if !list.is_empty() => Some(
            list.into_iter()
                .filter(|a| a.available)
                .map(|a| a.name)
                .collect(),
        ),
        _ => None,
    };
    ProfileConnect {
        status: app::ServerStatus::Live,
        client: Some(client),
        sessions,
        last_error: None,
        agent_availability,
    }
}

/// Resolve TLS trust for a named profile *non-interactively*. Mirrors
/// [`establish_trust`] but never prompts — the fanout caller treats
/// missing pins as "unreachable" instead of asking the user to
/// confirm every server at startup.
fn trust_for_profile(base: &Url, profile: &profiles::Profile) -> Result<api::TlsTrust> {
    if base.scheme() == "http" {
        return Ok(api::TlsTrust::Plain);
    }
    if profile.insecure {
        return Ok(api::TlsTrust::AcceptAny);
    }
    if let Some(raw) = &profile.fingerprint {
        let fp = trust::normalize_fingerprint(raw)
            .with_context(|| format!("invalid fingerprint for profile: {raw}"))?;
        return Ok(api::TlsTrust::Pinned(fp));
    }
    let host_key = trust::host_key(base)?;
    if let Some(pinned) = trust::KnownHosts::load()?.pin(&host_key) {
        return Ok(api::TlsTrust::Pinned(pinned.to_string()));
    }
    bail!(
        "no pinned fingerprint for {host_key} — connect once with `agentum terminal --profile <name>` interactively"
    );
}

/// Layer profile defaults under any explicit CLI flags. Returns the
/// merged `Options` so the rest of `run()` reads from one source. A
/// missing profile name resolves to the file's `default = …`; an
/// explicit `--profile` that doesn't exist is an error so the user
/// notices typos instead of silently dropping back to the loopback.
fn apply_profile(mut opts: Options) -> Result<Options> {
    let profiles = match profiles::Profiles::load() {
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

    let active = match opts.profile.clone() {
        Some(name) => Some(name),
        None => profiles.default_name().map(str::to_string),
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
        return Url::parse(&s).with_context(|| format!("invalid --api URL: {s}"));
    }
    // Loopback discovery only — we never silently autodial a remote.
    for candidate in [DEFAULT_HTTPS, DEFAULT_HTTP] {
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
        "no agentum daemon found on loopback. Start one with `agentum serve` or pass --api https://<host>:<port>"
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
    let target = opts
        .api
        .clone()
        .unwrap_or_else(|| "loopback (127.0.0.1:8822)".to_string());
    eprintln!();
    eprintln!("  agentum couldn't reach a daemon at {target}.");
    eprintln!("  ↳ {err}");
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

/// Walk the user through creating + saving a profile, returning the
/// chosen name. Reuses [`profiles::Profiles`] so the TUI subcommand,
/// CLI subcommand, and dashboard switcher all share one storage shape.
fn interactive_add_profile() -> Result<String> {
    eprintln!();
    eprintln!("  Add a new server:");
    let name = read_line("    name (e.g. vps): ")?;
    if name.trim().is_empty() {
        bail!("name is required");
    }
    let url = read_line("    URL (e.g. https://my-vps.example.com:8822): ")?;
    if url.trim().is_empty() {
        bail!("URL is required");
    }
    Url::parse(url.trim()).with_context(|| format!("invalid URL: {url}"))?;
    let fp_raw = read_line("    fingerprint (optional, AB:CD:…): ")?;
    let fingerprint = if fp_raw.trim().is_empty() {
        None
    } else {
        Some(
            trust::normalize_fingerprint(fp_raw.trim())
                .with_context(|| format!("invalid fingerprint: {fp_raw}"))?,
        )
    };
    let make_default_raw = read_line("    set as default? [y/N]: ")?;
    let set_default = matches!(make_default_raw.trim(), "y" | "Y" | "yes");

    let mut store = profiles::Profiles::load().context("load profiles.toml")?;
    store
        .upsert(
            name.trim().to_string(),
            profiles::Profile {
                url: url.trim().to_string(),
                fingerprint,
                insecure: false,
            },
        )
        .with_context(|| format!("save profile `{}`", name.trim()))?;
    if set_default {
        store
            .set_default(Some(name.trim().to_string()))
            .with_context(|| format!("mark `{}` default", name.trim()))?;
    }
    eprintln!(
        "  saved profile `{}` to {}",
        name.trim(),
        store.path().display()
    );
    eprintln!();
    Ok(name.trim().to_string())
}

fn read_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush().ok();
    let mut buf = String::new();
    let n = io::stdin().lock().read_line(&mut buf)?;
    if n == 0 {
        bail!("unexpected EOF on stdin");
    }
    Ok(buf.trim_end_matches(['\r', '\n']).to_string())
}
