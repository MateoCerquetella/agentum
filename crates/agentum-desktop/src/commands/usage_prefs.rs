//! Tiny persisted per-provider enable flag for usage scanning. Local JSON at
//! `$HOME/.agentum/usage-prefs.json`; absent ⇒ caller's default (true for
//! Claude/Codex so the dashboard shows data on first open).
use std::path::PathBuf;

pub fn prefs_path() -> Option<PathBuf> {
    // AGENTUM_HOME is the project-wide test-isolation env var; when set it
    // already points at the .agentum root directly. Fall back to $HOME/.agentum.
    if let Some(base) = std::env::var_os("AGENTUM_HOME").map(PathBuf::from) {
        return Some(base.join("usage-prefs.json"));
    }
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
        // Serialize against the other AGENTUM_HOME-mutating test (accounts):
        // both set this process-global var, and parallel test threads would
        // otherwise race (our TempDir can be dropped+deleted out from under the
        // other test). Hold the guard for the whole body so no other such test
        // runs while our TempDir is the active AGENTUM_HOME.
        let _home_guard = crate::commands::ENV_HOME_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Isolate via AGENTUM_HOME (project-wide test-isolation var) so the real
        // prefs file is never touched and we don't mutate process-global $HOME.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("AGENTUM_HOME", tmp.path());
        assert!(provider_enabled("claude", true)); // default = true
        assert!(!provider_enabled("claude", false)); // default = false honored when no file
        set_provider_enabled("claude", false);
        assert!(!provider_enabled("claude", true)); // persisted false wins over default
        set_provider_enabled("claude", true);
        assert!(provider_enabled("claude", false));
    }
}
