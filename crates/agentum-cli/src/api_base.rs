//! Resolve which agentum-server an `agentum` subcommand should talk to.
//!
//! Precedence, highest first:
//! 1. `$AGENTUM_API_URL` — injected into every pane the desktop's embedded
//!    server spawns (it binds an ephemeral loopback port, so this is the ONLY
//!    way an in-pane CLI can find that exact server). This is what makes the
//!    capability-card skills work: a skill command run inside an agentum pane
//!    drives the same control plane the desktop is showing.
//! 2. The active connection profile's URL (`profiles.toml`), for a CLI run
//!    outside a pane that targets a configured (possibly remote) daemon.
//! 3. A running desktop's advertised loopback port (its boot writes one to
//!    `state/desktop-api.url`; we probe it for liveness). This lets the
//!    capability-card example prompts work when pasted into an agent in the
//!    user's OWN terminal — no `$AGENTUM_API_URL`, no profile — while the
//!    desktop is open. Without it those calls hit the TLS daemon below and fail.
//! 4. `http://127.0.0.1:8822` — the conventional standalone-daemon address.

const DEFAULT_BASE: &str = "http://127.0.0.1:8822";

/// Pure resolver: pick the base URL from the (already-read) env var and active
/// profile URL. Kept side-effect-free so it can be unit-tested without touching
/// the environment or the filesystem.
pub fn resolve_api_base_from(env_url: Option<String>, profile_url: Option<String>) -> String {
    resolve_api_base_with(env_url, profile_url, None)
}

/// As [`resolve_api_base_from`], plus a third fallback: a discovered desktop URL
/// (already liveness-checked), tried after env/profile but before the
/// conventional `:8822` default. Pure so the precedence is unit-testable; the
/// impure discovery + probe lives in [`discovered_desktop_base`].
pub fn resolve_api_base_with(
    env_url: Option<String>,
    profile_url: Option<String>,
    discovered_url: Option<String>,
) -> String {
    let nonblank = |s: &String| !s.trim().is_empty();
    env_url
        .filter(nonblank)
        .or(profile_url.filter(nonblank))
        .or(discovered_url.filter(nonblank))
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// Resolve the base URL from the real environment + profiles file + a running
/// desktop. Precedence: `$AGENTUM_API_URL` (in-pane) → active profile → a live
/// desktop's advertised loopback port → `127.0.0.1:8822`. The desktop step is
/// what lets `agentum tab/computer/browser/status` work from ANY shell while the
/// desktop app is open — not just a pane the desktop itself spawned.
pub fn resolve_api_base() -> String {
    let env_url = std::env::var("AGENTUM_API_URL").ok();
    let profile_url = active_profile_url();
    // env/profile are explicit intent and win without a network round trip;
    // only probe for a live desktop when neither is set.
    let explicit = env_url.as_deref().is_some_and(|s| !s.trim().is_empty())
        || profile_url.as_deref().is_some_and(|s| !s.trim().is_empty());
    let discovered = if explicit {
        None
    } else {
        discovered_desktop_base()
    };
    resolve_api_base_with(env_url, profile_url, discovered)
}

/// The desktop's advertised base URL, but only when something is actually
/// listening there. A stale advertisement (desktop quit, or an OS-recycled
/// ephemeral port) must NOT shadow the conventional daemon address, so we open a
/// short-timeout TCP connection before trusting it. Loopback only, so success is
/// sub-millisecond and failure is a fast connection refusal.
fn discovered_desktop_base() -> Option<String> {
    let url = agentum_store::discovery::advertised_desktop_api_url()?;
    probe_alive(&url).then_some(url)
}

/// Is a loopback server listening at `url`'s authority? Parses the `host:port`
/// out of an `http://127.0.0.1:<port>` URL (the only shape the desktop writes)
/// and attempts a brief TCP connect.
fn probe_alive(url: &str) -> bool {
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
        .trim_end_matches('/');
    let Ok(addr) = authority.parse::<std::net::SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

/// The active (default) connection profile's URL, if profiles load and a
/// default is set. Best-effort: a missing/unreadable file yields `None`.
fn active_profile_url() -> Option<String> {
    let profiles = crate::commands::terminal::profiles::load().ok()?;
    let name = profiles.default_name()?.to_string();
    profiles.get(&name).map(|p| p.url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_url_wins_over_profile_and_default() {
        let got = resolve_api_base_from(
            Some("http://127.0.0.1:9001".into()),
            Some("https://vps:8822".into()),
        );
        assert_eq!(got, "http://127.0.0.1:9001");
    }

    #[test]
    fn profile_url_used_when_env_absent() {
        assert_eq!(
            resolve_api_base_from(None, Some("https://vps:8822".into())),
            "https://vps:8822"
        );
    }

    #[test]
    fn falls_back_to_default_when_both_unset() {
        assert_eq!(resolve_api_base_from(None, None), "http://127.0.0.1:8822");
    }

    #[test]
    fn blank_env_is_ignored() {
        // An empty AGENTUM_API_URL (e.g. `export AGENTUM_API_URL=`) must not
        // shadow the profile/default with a useless empty base.
        assert_eq!(
            resolve_api_base_from(Some("   ".into()), Some("https://vps:8822".into())),
            "https://vps:8822"
        );
    }

    #[test]
    fn discovered_used_when_env_and_profile_absent() {
        // The case the whole feature exists for: no in-pane env, no profile, but
        // a desktop is up — drive it instead of the TLS daemon default.
        assert_eq!(
            resolve_api_base_with(None, None, Some("http://127.0.0.1:50990".into())),
            "http://127.0.0.1:50990"
        );
    }

    #[test]
    fn env_and_profile_win_over_discovered() {
        assert_eq!(
            resolve_api_base_with(
                Some("http://127.0.0.1:9001".into()),
                None,
                Some("http://127.0.0.1:50990".into()),
            ),
            "http://127.0.0.1:9001"
        );
        assert_eq!(
            resolve_api_base_with(
                None,
                Some("https://vps:8822".into()),
                Some("http://127.0.0.1:50990".into()),
            ),
            "https://vps:8822"
        );
    }

    #[test]
    fn blank_or_absent_discovered_falls_back_to_default() {
        assert_eq!(
            resolve_api_base_with(None, None, Some("  ".into())),
            DEFAULT_BASE
        );
        assert_eq!(resolve_api_base_with(None, None, None), DEFAULT_BASE);
    }

    #[test]
    fn probe_alive_tracks_a_loopback_listener_lifecycle() {
        use std::net::TcpListener;
        // Bind an ephemeral loopback port: while it's open the probe must see it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        assert!(probe_alive(&url), "a bound listener should look alive");
        // Once dropped, the port refuses — a stale advertisement is rejected.
        drop(listener);
        assert!(
            !probe_alive(&url),
            "after the listener is dropped the port should refuse"
        );
    }

    #[test]
    fn probe_alive_rejects_unparseable_authorities() {
        assert!(!probe_alive("not a url"));
        assert!(!probe_alive("http://"));
        assert!(!probe_alive("http://localhost:notaport"));
    }
}
