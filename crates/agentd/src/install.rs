//! `agentd install` and `agentd status`.
use crate::config::Config;
use crate::util::{home, is_root};
use anyhow::{Context, bail};
use std::path::PathBuf;
use std::process::Command;

pub struct InstallArgs {
    pub hub: String,
    pub token: String,
    pub name: String,
    pub ca: Option<String>,
}

pub fn unit_path() -> PathBuf {
    if is_root() { PathBuf::from("/etc/systemd/system/agentd.service") } else { home().join(".config/systemd/user/agentd.service") }
}

/// The systemd unit: this binary, `run`, with the PATH of the shell that installed it.
pub fn unit_text(exe: &str, path_env: &str, system: bool) -> String {
    let mut u = String::from("[Unit]\nDescription=Agentum agentd (coding agents, terminals and git for the hub)\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\n");
    u.push_str(&format!("ExecStart={exe} run\n"));
    u.push_str("Restart=always\nRestartSec=2\n");
    u.push_str(&format!("Environment=PATH={path_env}\n"));
    if system {
        u.push_str(&format!("Environment=HOME={}\n", home().display()));
    }
    u.push_str("\n[Install]\n");
    u.push_str(if system { "WantedBy=multi-user.target\n" } else { "WantedBy=default.target\n" });
    u
}

fn systemctl(system: bool, args: &[&str]) -> anyhow::Result<()> {
    let mut c = Command::new("systemctl");
    if !system {
        c.arg("--user");
    }
    let st = c.args(args).status().context("running systemctl")?;
    if !st.success() {
        bail!("systemctl {} failed ({st})", args.join(" "));
    }
    Ok(())
}

pub fn install(a: InstallArgs) -> anyhow::Result<()> {
    if !(a.hub.starts_with("ws://") || a.hub.starts_with("wss://")) {
        bail!("--hub must be a ws:// or wss:// URL (like wss://hub.example/agentd)");
    }
    if let Some(ca) = &a.ca {
        let p = crate::util::expand(ca);
        crate::hub::tls_config(Some(&p.to_string_lossy())).with_context(|| format!("checking --ca {}", p.display()))?;
    }
    let mut cfg = Config::load_or_default();
    cfg.hub = a.hub;
    cfg.token = a.token;
    cfg.name = a.name;
    if a.ca.is_some() {
        cfg.ca = a.ca;
    }
    let cp = cfg.save()?;
    println!("wrote {}", cp.display());
    std::fs::create_dir_all(cfg.data_dir())?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let system = is_root();
    let path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    let unit = unit_path();
    std::fs::create_dir_all(unit.parent().unwrap())?;
    std::fs::write(&unit, unit_text(&exe.to_string_lossy(), &path_env, system))?;
    println!("wrote {}", unit.display());
    if std::env::var_os("AGENTD_NO_SYSTEMCTL").is_some() {
        return Ok(());
    }
    systemctl(system, &["daemon-reload"])?;
    systemctl(system, &["enable", "agentd"])?;
    // `restart` so a reinstall picks up the new config and binary.
    systemctl(system, &["restart", "agentd"])?;
    println!("agentd is running ({} unit). `agentd status` shows how it is doing.", if system { "system" } else { "user" });
    if !system {
        let user = std::env::var("USER").unwrap_or_default();
        if !user.is_empty() && !PathBuf::from(format!("/var/lib/systemd/linger/{user}")).exists() {
            println!("note: to keep it running while you are logged out: sudo loginctl enable-linger {user}");
        }
    }
    Ok(())
}

pub fn status() -> anyhow::Result<()> {
    let p = Config::path();
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            println!("not installed: {e:#}");
            return Ok(());
        }
    };
    println!("config   {}", p.display());
    println!("hub      {}", cfg.hub);
    println!("name     {}", cfg.name);
    println!("data     {}", cfg.data_dir().display());
    let system = is_root();
    let mut c = Command::new("systemctl");
    if !system {
        c.arg("--user");
    }
    if let Ok(o) = c.args(["is-active", "agentd"]).output() {
        println!("service  {}", String::from_utf8_lossy(&o.stdout).trim());
    }
    let sp = crate::App::status_path(&cfg);
    match std::fs::read_to_string(&sp).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        Some(s) => {
            let pid = s["pid"].as_u64().unwrap_or(0);
            // SAFETY: signal 0 only checks that the process exists.
            let alive = pid > 0 && unsafe { libc::kill(pid as i32, 0) } == 0;
            if !alive {
                println!("daemon   not running (last seen pid {pid})");
                return Ok(());
            }
            let since = s["since"].as_i64().map(|t| format!(" for {}", ago(crate::util::now_ms() - t))).unwrap_or_default();
            println!("daemon   pid {pid}, v{}, {}{since}", s["version"].as_str().unwrap_or("?"), if s["connected"].as_bool() == Some(true) { "connected" } else { "not connected" });
            println!("         {} live agent processes, {} terminals, {} projects, {} MB", s["threads_live"], s["terminals"], s["projects"], s["rss_mb"]);
        }
        None => println!("daemon   no status yet ({})", sp.display()),
    }
    Ok(())
}

fn ago(ms: i64) -> String {
    let s = ms / 1000;
    if s < 120 {
        format!("{s}s")
    } else if s < 7200 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_has_path_and_run() {
        let u = super::unit_text("/opt/agentd", "/home/me/.local/bin:/usr/bin", false);
        assert!(u.contains("ExecStart=/opt/agentd run\n"));
        assert!(u.contains("Environment=PATH=/home/me/.local/bin:/usr/bin\n"));
        assert!(u.contains("WantedBy=default.target"));
        assert!(super::unit_text("/a", "/b", true).contains("WantedBy=multi-user.target"));
    }
}
