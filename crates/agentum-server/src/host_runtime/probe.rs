//! Host capability probing and readiness assembly.
use super::*;

/// Raw output of the preflight, before it's shaped into a
/// [`HostReadiness`]. `bins` maps a probed binary name to its resolved
/// path; an empty string means "not found".
#[derive(Debug)]
pub(crate) struct ProbeOutput {
    pub(crate) uname: Option<String>,
    pub(crate) pkg_manager: String,
    /// `Some(true)` if `sudo -n true` succeeded on the remote (passwordless
    /// sudo / root); `Some(false)` if it failed; `None` if undetermined.
    pub(crate) sudo_nopasswd: Option<bool>,
    pub(crate) bins: HashMap<String, String>,
}

/// The binaries to probe, deduped, required deps first then agent CLIs.
/// `cursor` and `agent` may both map to distinct binaries; dedup keeps a
/// single `command -v` per binary regardless of how many tool ids share
/// it.
pub(crate) fn probe_binaries() -> Vec<String> {
    let mut bins: Vec<String> = Vec::new();
    let mut push = |b: String| {
        if !bins.contains(&b) {
            bins.push(b);
        }
    };
    for d in REQUIRED_DEPS {
        push(d.to_string());
    }
    for tool in probed_tools() {
        push(binary_for(tool).to_string());
    }
    bins
}

/// Build the structured report from raw probe data. Sets `ok` from the
/// required deps only; agent gaps never block here (the New Session form
/// blocks only when the *picked* tool is unavailable). Install hints are
/// left empty — [`crate::host_install_hints::fill_hints`] fills them.
pub(crate) fn assemble_readiness(probe: ProbeOutput) -> HostReadiness {
    let required: Vec<DepCheck> = REQUIRED_DEPS
        .iter()
        .map(|id| {
            let installed = probe.bins.get(*id).map(|p| !p.is_empty()).unwrap_or(false);
            DepCheck {
                id: (*id).to_string(),
                label: (*id).to_string(),
                installed,
                install_hint: None,
                bootstrapable: false,
            }
        })
        .collect();

    let agents: Vec<AgentDepCheck> = probed_tools()
        .map(|tool| {
            let binary = binary_for(tool);
            let path = probe.bins.get(binary).filter(|p| !p.is_empty()).cloned();
            AgentDepCheck {
                id: tool.to_string(),
                binary: binary.to_string(),
                installed: path.is_some(),
                path,
                install_hint: None,
                bootstrapable: false,
            }
        })
        .collect();

    let missing: Vec<&str> = required
        .iter()
        .filter(|d| !d.installed)
        .map(|d| d.id.as_str())
        .collect();
    let ok = missing.is_empty();
    let message = if ok {
        "host ready".to_string()
    } else {
        format!(
            "{} required {} missing: {}",
            missing.len(),
            if missing.len() == 1 {
                "dependency"
            } else {
                "dependencies"
            },
            missing.join(", ")
        )
    };

    HostReadiness {
        ok,
        message,
        system: HostSystemInfo {
            uname: probe.uname,
            pkg_manager: probe.pkg_manager,
            sudo_nopasswd: probe.sudo_nopasswd,
        },
        required,
        agents,
        // Filled by `readiness()` after assembly (needs an async host probe);
        // empty here and for the unreachable path.
        skills: Vec::new(),
    }
}

/// Readiness for a host we couldn't reach. Everything reports missing;
/// `message` carries the SSH error.
pub(crate) fn unreachable_readiness(message: String) -> HostReadiness {
    let mut report = assemble_readiness(ProbeOutput {
        uname: None,
        pkg_manager: "unknown".to_string(),
        sudo_nopasswd: None,
        bins: HashMap::new(),
    });
    report.message = message;
    report
}

/// Local-host probe: synchronous `which` lookups, no SSH.
pub(crate) fn probe_local() -> ProbeOutput {
    let uname = std::process::Command::new("uname")
        .arg("-sr")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut bins = HashMap::new();
    for b in probe_binaries() {
        let path = which::which(&b)
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        bins.insert(b, path);
    }
    // Local passwordless-sudo check mirrors the remote `sudo -n true`.
    let sudo_nopasswd = Some(
        std::process::Command::new("sudo")
            .arg("-n")
            .arg("true")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
    );
    ProbeOutput {
        uname,
        pkg_manager: detect_local_pkg_manager(),
        sudo_nopasswd,
        bins,
    }
}

fn detect_local_pkg_manager() -> String {
    for (bin, name) in [
        ("apt-get", "apt"),
        ("dnf", "dnf"),
        ("pacman", "pacman"),
        ("brew", "brew"),
    ] {
        if which::which(bin).is_ok() {
            return name.to_string();
        }
    }
    "unknown".to_string()
}

/// SSH probe: one round trip running a POSIX `sh -c` script that prints
/// `uname`, the package manager, and a `command -v` line per binary. We
/// wrap in `sh -c` because the remote login shell may be fish/zsh, which
/// don't share bash's `for`/`case` syntax — `sh -c` forces POSIX.
pub(crate) async fn probe_ssh(host: &Host) -> Result<ProbeOutput> {
    let inner = build_probe_script(&probe_binaries())?;
    let script = format!("sh -c {}", q(&inner)?);
    let stdout = ssh_stdout(host, &script).await?;
    Ok(parse_probe_output(&stdout))
}

pub(crate) fn build_probe_script(bins: &[String]) -> Result<String> {
    let mut s = String::new();
    s.push_str(r#"printf 'uname\t%s\n' "$(uname -sr 2>/dev/null)"; "#);
    s.push_str(
        r#"pm=unknown; for c in apt-get dnf pacman brew; do if command -v "$c" >/dev/null 2>&1; then case "$c" in apt-get) pm=apt;; *) pm="$c";; esac; break; fi; done; "#,
    );
    s.push_str(r#"printf 'pkg\t%s\n' "$pm"; "#);
    // Passwordless-sudo check: `sudo -n true` never prompts. Reports yes
    // only when sudo runs without a password (or the user is root).
    s.push_str(
        r#"if sudo -n true >/dev/null 2>&1; then printf 'sudo\tyes\n'; else printf 'sudo\tno\n'; fi; "#,
    );
    for b in bins {
        let qb = q(b)?;
        s.push_str(&format!(
            r#"printf 'bin\t%s\t%s\n' {qb} "$(command -v {qb} 2>/dev/null)"; "#
        ));
    }
    Ok(s)
}

/// Parse the tab-delimited preflight output. Tolerant of trailing `\r`,
/// blank lines, and unknown keys (forward-compat).
pub(crate) fn parse_probe_output(stdout: &str) -> ProbeOutput {
    let mut out = ProbeOutput {
        uname: None,
        pkg_manager: "unknown".to_string(),
        sudo_nopasswd: None,
        bins: HashMap::new(),
    };
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        let mut it = line.splitn(2, '\t');
        let key = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        match key {
            "uname" => {
                let v = rest.trim();
                if !v.is_empty() {
                    out.uname = Some(v.to_string());
                }
            }
            "sudo" => out.sudo_nopasswd = Some(rest.trim() == "yes"),
            "pkg" => {
                let v = rest.trim();
                out.pkg_manager = if v.is_empty() {
                    "unknown".to_string()
                } else {
                    v.to_string()
                };
            }
            "bin" => {
                let mut bit = rest.splitn(2, '\t');
                let name = bit.next().unwrap_or("").trim();
                // The path may legitimately contain spaces; only trim the
                // surrounding whitespace, never split it.
                let path = bit.next().unwrap_or("").trim();
                if !name.is_empty() {
                    out.bins.insert(name.to_string(), path.to_string());
                }
            }
            _ => {}
        }
    }
    out
}
