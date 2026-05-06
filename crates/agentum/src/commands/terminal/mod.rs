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
pub mod app;
mod extensions;
mod palette;
mod pty;
mod term;
mod theme;
pub mod trust;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
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
}

pub async fn run(opts: Options) -> Result<()> {
    let base = resolve_base(opts.api.clone()).await?;
    let trust = establish_trust(&base, &opts).await?;
    let token = obtain_token(&base, &trust).await?;

    let client = api::Client::new(base.clone(), token, trust)?;
    client.health().await.with_context(|| {
        format!("agentum daemon not reachable at {base} — start it with `agentum serve`")
    })?;

    let sessions = client
        .list_sessions()
        .await
        .context("failed to list sessions")?;

    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let _restore = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("init ratatui terminal")?;

    app::run_loop(&mut terminal, client, sessions).await
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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
    eprintln!("Log in to agentum at {base}");
    eprint!("  username: ");
    io::stderr().flush().ok();
    let mut user = String::new();
    io::stdin().read_line(&mut user).context("read username")?;
    let user = user.trim().to_string();

    eprint!("  password: ");
    io::stderr().flush().ok();
    let mut pw = String::new();
    io::stdin().read_line(&mut pw).context("read password")?;
    let pw = pw.trim_end_matches(['\n', '\r']).to_string();

    let token = api::login(base, trust_setting, &user, &pw)
        .await
        .context("login failed")?;
    creds.put(host_key, token.clone(), Some(user))?;
    Ok(token)
}

async fn probe_token(base: &Url, trust_setting: &TlsTrust, token: &str) -> bool {
    let Ok(client) = api::Client::new(base.clone(), token.to_string(), trust_setting.clone())
    else {
        return false;
    };
    client.me().await.is_ok()
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
