//! Tiny persisted per-provider enable flag for usage scanning. Local JSON at
//! `$HOME/.agentum/usage-prefs.json`; absent ⇒ caller's default (true for
//! Claude/Codex so the dashboard shows data on first open).
use std::path::PathBuf;

pub fn prefs_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".agentum").join("usage-prefs.json"))
}

fn load() -> serde_json::Value {
    prefs_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn provider_enabled(provider: &str, default: bool) -> bool {
    load()
        .get(provider)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

pub fn set_provider_enabled(provider: &str, enabled: bool) {
    let mut cfg = load();
    cfg[provider] = serde_json::Value::Bool(enabled);
    if let Some(path) = prefs_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_when_absent_then_roundtrips() {
        // Isolate HOME to a temp dir so the real prefs file is never touched.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        assert!(provider_enabled("claude", true)); // default = true
        assert!(!provider_enabled("claude", false)); // default = false honored when no file
        set_provider_enabled("claude", false);
        assert!(!provider_enabled("claude", true)); // persisted false wins over default
        set_provider_enabled("claude", true);
        assert!(provider_enabled("claude", false));
    }
}
