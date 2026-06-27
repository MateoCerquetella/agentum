//! Browser navigation security policy and [browser] config loading.
use super::*;

// --- navigation security (§9) ------------------------------------------------

/// `scheme://host[:port]` origin of a url, lowercased, or `None` (e.g. `data:`).
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

/// Block reason for a navigation target, or `None` if allowed. `file://` is always
/// blocked. `allowed_origins`: `None` or `"*"`/empty = allow all (local-dev
/// default); a comma list = allow only those origins (deny-by-default). Pure.
pub(crate) fn navigation_block_reason(url: &str, allowed_origins: Option<&str>) -> Option<String> {
    if url.trim().to_ascii_lowercase().starts_with("file:") {
        return Some("file:// navigation is blocked".to_string());
    }
    match allowed_origins.map(str::trim) {
        None | Some("") | Some("*") => None,
        Some(list) => {
            let origin = origin_of(url);
            let allowed = list
                .split(',')
                .map(str::trim)
                .any(|a| !a.is_empty() && origin.as_deref() == Some(a));
            if allowed {
                None
            } else {
                Some(format!(
                    "origin not in allowed_origins: {}",
                    origin.unwrap_or_else(|| url.to_string())
                ))
            }
        }
    }
}

// --- [browser] config (§10) --------------------------------------------------

/// Wire shape for `<config_dir>/browser.toml`'s `[browser]` section. Only the
/// behaviorally-wired knobs are typed; serde ignores the rest (driver, render_mode,
/// viewport, screencast) so a full §10 file still parses (forward-compat) without
/// dead fields here — those are consumed by cdp_browser/cdp_screencast via env.
#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(default)]
struct BrowserConfigFile {
    browser: BrowserSection,
}

#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(default)]
struct BrowserSection {
    allow_eval: bool,
    allowed_origins: Vec<String>,
    nav_timeout_ms: Option<u64>,
}

/// Cached `[browser]` config (read once; changing it needs a restart, matching the
/// other agentum singletons). Missing/invalid file → defaults.
fn browser_config() -> &'static BrowserSection {
    static CFG: OnceLock<BrowserSection> = OnceLock::new();
    CFG.get_or_init(load_browser_config)
}

fn load_browser_config() -> BrowserSection {
    let Ok(dir) = agentum_store::paths::config_dir() else {
        return BrowserSection::default();
    };
    let Ok(raw) = std::fs::read_to_string(dir.join("browser.toml")) else {
        return BrowserSection::default();
    };
    toml::from_str::<BrowserConfigFile>(&raw)
        .map(|f| f.browser)
        .unwrap_or_default()
}

/// Map a configured origin list to the policy string used by
/// [`navigation_block_reason`]: empty or containing `*` → allow all (`None`).
pub(crate) fn origins_to_policy(list: &[String]) -> Option<String> {
    if list.is_empty() || list.iter().any(|o| o == "*") {
        None
    } else {
        Some(list.join(","))
    }
}

/// Allowed-origins policy: env override, else `[browser].allowed_origins`, else
/// allow-all (local dev).
pub(crate) fn allowed_origins() -> Option<String> {
    if let Ok(v) = std::env::var("AGENTUM_BROWSER_ALLOWED_ORIGINS") {
        return if v.trim() == "*" || v.trim().is_empty() {
            None
        } else {
            Some(v)
        };
    }
    origins_to_policy(&browser_config().allowed_origins)
}

/// Whether `browser_eval` is enabled — OFF by default (§9). env override, else
/// `[browser].allow_eval`.
pub(crate) fn eval_allowed() -> bool {
    if let Ok(v) = std::env::var("AGENTUM_BROWSER_ALLOW_EVAL") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    browser_config().allow_eval
}

/// Navigation load-wait timeout: `[browser].nav_timeout_ms`, default 15s.
pub(crate) fn nav_timeout_ms() -> u64 {
    browser_config().nav_timeout_ms.unwrap_or(15_000)
}
