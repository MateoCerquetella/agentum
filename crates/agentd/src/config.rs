//! `~/.config/agentum/agentd.toml` (or `AGENTD_*` variables, which override it) and the data directory.
use crate::util::{expand, home};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    /// The hub's agentd endpoint, e.g. `wss://hub.example.lan/agentd`.
    pub hub: String,
    /// This machine's token (sent as `Authorization: Bearer <token>`).
    pub token: String,
    /// The machine's name.
    pub name: String,
    /// A PEM file of a private CA to trust for `wss://` (besides the public roots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    /// Connect to this address instead of resolving the hub's host name (TLS still checks the name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_ip: Option<String>,
    /// Directories scanned for git repos (depth ≤ 4). Default: ~/Developer, ~/projects, ~/src, ~/code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<String>>,
    /// Extra repo paths, always listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// Minutes a thread may sit idle before its agent process is ended (it resumes on the next send).
    #[serde(default = "default_idle", skip_serializing_if = "is_default_idle")]
    pub idle_minutes: u64,
    /// Where the database and worktrees live. Default: ~/.local/share/agentum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The agent CLIs, when not on PATH (or to run another build).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<String>,
}

fn default_idle() -> u64 {
    10
}
fn is_default_idle(v: &u64) -> bool {
    *v == 10
}

pub const DEFAULT_ROOTS: [&str; 4] = ["~/Developer", "~/projects", "~/src", "~/code"];

impl Config {
    /// `$AGENTD_CONFIG`, else `~/.config/agentum/agentd.toml`.
    pub fn path() -> PathBuf {
        match std::env::var_os("AGENTD_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => home().join(".config/agentum/agentd.toml"),
        }
    }

    /// The file, then the `AGENTD_*` variables over it. Without a file, the variables alone
    /// (`AGENTD_HUB` at least).
    pub fn load() -> anyhow::Result<Config> {
        let p = Self::path();
        let mut c: Config = match std::fs::read_to_string(&p) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && std::env::var_os("AGENTD_HUB").is_some() => Config { idle_minutes: default_idle(), ..Default::default() },
            Err(e) => return Err(e).with_context(|| format!("reading {} (run `agentd install` first)", p.display())),
        };
        c.apply_env(|k| std::env::var(k).ok());
        anyhow::ensure!(!c.hub.is_empty(), "{}: `hub` is empty (and AGENTD_HUB isn't set)", p.display());
        Ok(c)
    }

    /// `AGENTD_HUB`, `AGENTD_TOKEN`, `AGENTD_NAME`, `AGENTD_CA`, `AGENTD_HUB_IP`, `AGENTD_ROOTS` and
    /// `AGENTD_PROJECTS` (lists separated by `:` or `,`) override the file; a variable that is set
    /// wins even when empty (`AGENTD_ROOTS=` scans nothing).
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) {
        let list = |v: String| v.split([':', ',']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
        if let Some(v) = get("AGENTD_HUB") {
            self.hub = v;
        }
        if let Some(v) = get("AGENTD_TOKEN") {
            self.token = v;
        }
        if let Some(v) = get("AGENTD_NAME") {
            self.name = v;
        }
        if let Some(v) = get("AGENTD_CA") {
            self.ca = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("AGENTD_HUB_IP") {
            self.hub_ip = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("AGENTD_ROOTS") {
            self.roots = Some(list(v));
        }
        if let Some(v) = get("AGENTD_PROJECTS") {
            self.projects = list(v);
        }
    }

    /// `$AGENTD_ENV_FILE`, else `~/.config/agentum/agentd.env`.
    pub fn env_file() -> PathBuf {
        match std::env::var_os("AGENTD_ENV_FILE") {
            Some(p) => PathBuf::from(p),
            None => home().join(".config/agentum/agentd.env"),
        }
    }

    pub fn load_or_default() -> Config {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_else(|| Config { idle_minutes: default_idle(), ..Default::default() })
    }

    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        {
            use std::os::unix::fs::PermissionsExt;
            // The token is a secret.
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &p)?;
        Ok(p)
    }

    pub fn data_dir(&self) -> PathBuf {
        match &self.data {
            Some(d) => expand(d),
            None => home().join(".local/share/agentum"),
        }
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir().join("worktrees")
    }

    /// The scan roots that exist.
    pub fn roots(&self) -> Vec<PathBuf> {
        let list: Vec<String> = match &self.roots {
            Some(r) => r.clone(),
            None => DEFAULT_ROOTS.iter().map(|s| s.to_string()).collect(),
        };
        list.iter().map(|r| expand(r)).filter(|p| p.is_dir()).collect()
    }

    /// The configured binary for an agent, if any.
    pub fn agent_bin(&self, agent: agentum_proto::AgentKind) -> Option<&str> {
        use agentum_proto::AgentKind::*;
        match agent {
            ClaudeCode => self.claude.as_deref(),
            Codex => self.codex.as_deref(),
            Opencode => self.opencode.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_minimal() {
        let c: Config = toml::from_str("hub = \"ws://h/agentd\"\ntoken = \"t\"\nname = \"n\"\n").unwrap();
        assert_eq!(c.idle_minutes, 10);
        assert!(c.projects.is_empty());
        let back = toml::to_string_pretty(&c).unwrap();
        assert!(!back.contains("idle_minutes"));
    }

    #[test]
    fn env_overrides_the_file() {
        let mut c: Config = toml::from_str("hub = \"ws://h/agentd\"\ntoken = \"t\"\nname = \"n\"\nca = \"/ca.pem\"\nprojects = [\"/a\"]\n").unwrap();
        let env = [("AGENTD_TOKEN", "t2"), ("AGENTD_ROOTS", ""), ("AGENTD_PROJECTS", "/x:/y, /z"), ("AGENTD_CA", ""), ("AGENTD_HUB_IP", "10.0.0.2")];
        c.apply_env(|k| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()));
        assert_eq!((c.hub.as_str(), c.token.as_str(), c.name.as_str()), ("ws://h/agentd", "t2", "n"));
        assert_eq!(c.roots, Some(vec![]));
        assert_eq!(c.projects, vec!["/x", "/y", "/z"]);
        assert_eq!(c.ca, None);
        assert_eq!(c.hub_ip.as_deref(), Some("10.0.0.2"));
    }
}
