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
//! 3. `http://127.0.0.1:8822` — the conventional standalone-daemon address.

const DEFAULT_BASE: &str = "http://127.0.0.1:8822";

/// Pure resolver: pick the base URL from the (already-read) env var and active
/// profile URL. Kept side-effect-free so it can be unit-tested without touching
/// the environment or the filesystem.
pub fn resolve_api_base_from(env_url: Option<String>, profile_url: Option<String>) -> String {
    env_url
        .filter(|s| !s.trim().is_empty())
        .or(profile_url.filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// Resolve the base URL from the real environment + profiles file. Reads
/// `$AGENTUM_API_URL` and the active profile; any failure to load profiles is
/// treated as "no profile" so the env var / default still apply.
pub fn resolve_api_base() -> String {
    let env_url = std::env::var("AGENTUM_API_URL").ok();
    let profile_url = active_profile_url();
    resolve_api_base_from(env_url, profile_url)
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
}
