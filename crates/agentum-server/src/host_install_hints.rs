//! Static install hints for host readiness reports.
//!
//! Two flavours of hint:
//!
//! - **Required system deps** (`tmux`, `git`): a per-package install
//!   command derived from the host's detected package manager. These are
//!   `bootstrapable` — phase 2's `agentum hosts bootstrap` can run them
//!   after explicit confirmation.
//! - **Agent CLIs** (`claude`, `codex`, …): a static pointer (URL or
//!   one-liner). Agentum **never** auto-installs agent CLIs, so these are
//!   informational only and never `bootstrapable`.
//!
//! [`fill_hints`] is called after parsing the remote preflight JSON,
//! before the [`HostReadiness`] is returned to a client. Keeping the
//! hints server-side means a single source of truth for both the CLI
//! table and the TUI overlay. See `docs/plans/SSH_HOST_READINESS_PRD.md`.

use agentum_core::HostReadiness;

/// System packages the bootstrap path is allowed to install. Anything
/// not in this list is rejected by both `bootstrap_command` and the
/// (phase-2) bootstrap route — we never let a client install arbitrary
/// packages, only the two required deps.
pub const BOOTSTRAPABLE: &[&str] = &["tmux", "git"];

/// Runnable one-liner that installs `tool`'s CLI on a host. `None` for
/// tools we don't have a verified installer for (the caller then shows a
/// generic hint and won't offer one-key install). Commands are the
/// official installers, verified May 2026:
/// - npm: `claude`, `codex`, `gemini`
/// - `curl … | bash`: `cursor`/`agent` (installs `cursor-agent`),
///   `opencode`, `hermes` (NousResearch)
/// - pip: `aider`
pub fn agent_install_command(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "claude" => "npm install -g @anthropic-ai/claude-code",
        "codex" => "npm install -g @openai/codex",
        "gemini" => "npm install -g @google/gemini-cli",
        // Cursor's headless CLI installs as `cursor-agent`; both the
        // `cursor` and `agent` tool ids resolve to it.
        "cursor" | "agent" => "curl https://cursor.com/install -fsS | bash",
        "opencode" => "curl -fsSL https://opencode.ai/install | bash",
        "hermes" => {
            "curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh | bash"
        }
        "aider" => "python -m pip install aider-install && aider-install",
        _ => return None,
    })
}

/// Display hint for a missing agent: the runnable install command, or a
/// generic line for tools we don't have a verified installer for. Never a
/// bare missing entry with no guidance.
pub fn agent_install_hint(tool: &str) -> &'static str {
    agent_install_command(tool).unwrap_or("install the agent CLI and ensure its binary is on PATH")
}

/// Build a package-install command for the detected package manager.
/// Returns `None` for an unknown manager (the caller substitutes a
/// generic "use your package manager" line) or when `packages` is empty.
///
/// Used both for required-dep install hints (one package at a time) and
/// for the phase-2 bootstrap command (`tmux` + `git` together).
pub fn bootstrap_command(pkg_manager: &str, packages: &[&str]) -> Option<String> {
    if packages.is_empty() {
        return None;
    }
    let pkgs = packages.join(" ");
    let cmd = match pkg_manager {
        "apt" => format!("sudo apt-get install -y {pkgs}"),
        "dnf" => format!("sudo dnf install -y {pkgs}"),
        "pacman" => format!("sudo pacman -S --needed {pkgs}"),
        "brew" => format!("brew install {pkgs}"),
        _ => return None,
    };
    Some(cmd)
}

/// Populate `install_hint` and `bootstrapable` on a freshly-parsed
/// readiness report. Idempotent: installed deps get `install_hint =
/// None`, missing ones get a manager-specific (or generic) command.
pub fn fill_hints(readiness: &mut HostReadiness) {
    let pkg = readiness.system.pkg_manager.clone();
    for dep in &mut readiness.required {
        dep.bootstrapable = BOOTSTRAPABLE.contains(&dep.id.as_str());
        dep.install_hint = if dep.installed {
            None
        } else {
            Some(
                bootstrap_command(&pkg, &[dep.id.as_str()])
                    .unwrap_or_else(|| format!("install {} with your package manager", dep.id)),
            )
        };
    }
    for agent in &mut readiness.agents {
        // Agent CLIs are never auto-installable.
        agent.bootstrapable = false;
        agent.install_hint = if agent.installed {
            None
        } else {
            Some(agent_install_hint(&agent.id).to_string())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::{AgentDepCheck, DepCheck, HostSystemInfo};

    fn readiness(pkg: &str) -> HostReadiness {
        HostReadiness {
            ok: false,
            message: String::new(),
            system: HostSystemInfo {
                uname: Some("Linux 6.12".into()),
                pkg_manager: pkg.into(),
                sudo_nopasswd: Some(true),
            },
            required: vec![
                DepCheck {
                    id: "tmux".into(),
                    label: "tmux".into(),
                    installed: false,
                    install_hint: None,
                    bootstrapable: false,
                },
                DepCheck {
                    id: "git".into(),
                    label: "git".into(),
                    installed: true,
                    install_hint: None,
                    bootstrapable: false,
                },
            ],
            agents: vec![
                AgentDepCheck {
                    id: "claude".into(),
                    binary: "claude".into(),
                    installed: false,
                    path: None,
                    install_hint: None,
                    bootstrapable: false,
                },
                AgentDepCheck {
                    id: "codex".into(),
                    binary: "codex".into(),
                    installed: true,
                    path: Some("/usr/bin/codex".into()),
                    install_hint: None,
                    bootstrapable: false,
                },
            ],
            skills: vec![],
        }
    }

    #[test]
    fn bootstrap_command_per_manager() {
        assert_eq!(
            bootstrap_command("apt", &["tmux", "git"]).as_deref(),
            Some("sudo apt-get install -y tmux git")
        );
        assert_eq!(
            bootstrap_command("dnf", &["tmux", "git"]).as_deref(),
            Some("sudo dnf install -y tmux git")
        );
        assert_eq!(
            bootstrap_command("pacman", &["tmux", "git"]).as_deref(),
            Some("sudo pacman -S --needed tmux git")
        );
        assert_eq!(
            bootstrap_command("brew", &["tmux", "git"]).as_deref(),
            Some("brew install tmux git")
        );
        assert_eq!(bootstrap_command("unknown", &["tmux"]), None);
        assert_eq!(bootstrap_command("pacman", &[]), None);
    }

    #[test]
    fn fill_hints_marks_only_required_bootstrapable() {
        let mut r = readiness("pacman");
        fill_hints(&mut r);

        let tmux = &r.required[0];
        assert_eq!(tmux.id, "tmux");
        assert!(tmux.bootstrapable);
        assert_eq!(
            tmux.install_hint.as_deref(),
            Some("sudo pacman -S --needed tmux")
        );

        let git = &r.required[1];
        assert!(
            git.bootstrapable,
            "git is bootstrapable even when installed"
        );
        assert!(
            git.install_hint.is_none(),
            "installed dep gets no install hint"
        );

        for agent in &r.agents {
            assert!(!agent.bootstrapable, "agents are never bootstrapable");
        }
        let claude = &r.agents[0];
        assert!(claude.install_hint.is_some(), "missing agent gets a hint");
        let codex = &r.agents[1];
        assert!(codex.install_hint.is_none(), "installed agent gets no hint");
    }

    #[test]
    fn fill_hints_falls_back_to_generic_for_unknown_manager() {
        let mut r = readiness("unknown");
        fill_hints(&mut r);
        assert_eq!(
            r.required[0].install_hint.as_deref(),
            Some("install tmux with your package manager")
        );
    }
}
