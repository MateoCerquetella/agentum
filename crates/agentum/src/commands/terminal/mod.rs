//! `agentum terminal` — interactive terminal dashboard (also reachable as
//! the `lazyagentum` shim binary, and aliased as `agentum tui`).
//!
//! Thin client. Talks to a running `agentum serve` over the same HTTP/WS
//! API the Svelte SPA uses. It never opens the database or touches tmux
//! directly. The lazygit side pane spawns a *local* PTY independently.

mod api;
mod app;
mod extensions;
mod pty;
mod term;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use url::Url;

const DEFAULT_HTTPS: &str = "https://127.0.0.1:8822";
const DEFAULT_HTTP: &str = "http://127.0.0.1:8822";

pub async fn run(api_override: Option<String>) -> Result<()> {
    let base = resolve_base(api_override).await?;
    let token = obtain_token(&base).await?;

    let client = api::Client::new(base.clone(), token)?;
    client.health().await.with_context(|| {
        format!(
            "agentum daemon not reachable at {base} — start it with `agentum serve`"
        )
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

/// CLI-cached login token. Lives next to the SQLite db. Permission 0600.
fn cli_token_path() -> Result<std::path::PathBuf> {
    let dir = agentum_store::paths::data_dir()
        .map_err(|e| anyhow!("resolve data dir: {e}"))?;
    Ok(dir.join("cli_token"))
}

fn read_cached_token() -> Option<String> {
    let path = cli_token_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let t = raw.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn write_cached_token(token: &str) -> Result<()> {
    let path = cli_token_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{token}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(&path, perm)?;
    }
    Ok(())
}

/// Get a usable bearer token: try the cached one first, validate it, else
/// prompt for username/password and exchange via /api/auth/login.
async fn obtain_token(base: &Url) -> Result<String> {
    if let Some(t) = read_cached_token() {
        if probe_token(base, &t).await {
            return Ok(t);
        }
    }

    use std::io::{self, Write};
    eprintln!("Log in to agentum at {base}");
    eprint!("username: ");
    io::stderr().flush().ok();
    let mut user = String::new();
    io::stdin().read_line(&mut user).context("read username")?;
    let user = user.trim().to_string();

    eprint!("password: ");
    io::stderr().flush().ok();
    let mut pw = String::new();
    io::stdin().read_line(&mut pw).context("read password")?;
    let pw = pw.trim_end_matches(['\n', '\r']).to_string();

    let token = api::login(base, &user, &pw)
        .await
        .context("login failed")?;
    let _ = write_cached_token(&token);
    Ok(token)
}

async fn probe_token(base: &Url, token: &str) -> bool {
    let Ok(client) = api::Client::new(base.clone(), token.to_string()) else {
        return false;
    };
    client.me().await.is_ok()
}

async fn resolve_base(override_url: Option<String>) -> Result<Url> {
    if let Some(s) = override_url {
        return Url::parse(&s).with_context(|| format!("invalid --api URL: {s}"));
    }
    for candidate in [DEFAULT_HTTPS, DEFAULT_HTTP] {
        let parsed = Url::parse(candidate).expect("static URL");
        if api::probe_health(&parsed, Duration::from_millis(750))
            .await
            .is_ok()
        {
            return Ok(parsed);
        }
    }
    Ok(Url::parse(DEFAULT_HTTPS).expect("static URL"))
}
