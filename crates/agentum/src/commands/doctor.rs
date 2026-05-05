use std::net::TcpListener;

use agentum_store::paths;
use anyhow::Result;
use tokio::process::Command;

struct Check {
    label: &'static str,
    passed: bool,
    detail: String,
}

impl Check {
    fn ok(label: &'static str, detail: impl Into<String>) -> Self {
        Self { label, passed: true, detail: detail.into() }
    }
    fn fail(label: &'static str, detail: impl Into<String>) -> Self {
        Self { label, passed: false, detail: detail.into() }
    }
}

pub async fn run() -> Result<()> {
    let checks = vec![
        check_tmux().await,
        check_dir("data dir", paths::data_dir),
        check_dir("config dir", paths::config_dir),
        check_db().await,
        check_tls(),
        check_users().await,
        check_port(8822),
    ];

    println!();
    let mut failures = 0u32;
    for c in &checks {
        let icon = if c.passed { "\x1b[32m✓\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
        println!("  {:<12} {}  {}", c.label, icon, c.detail);
        if !c.passed {
            failures += 1;
        }
    }
    println!();

    if failures == 0 {
        println!("all checks passed");
    } else {
        println!("{failures} problem{} found", if failures == 1 { "" } else { "s" });
        std::process::exit(1);
    }
    Ok(())
}

async fn check_tmux() -> Check {
    match Command::new("tmux").arg("-V").output().await {
        Ok(out) if out.status.success() => {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Check::ok("tmux", ver)
        }
        Ok(_) => Check::fail("tmux", "tmux found but returned an error"),
        Err(_) => Check::fail("tmux", "not found \u{2014} install with: apt install tmux"),
    }
}

fn check_dir(
    label: &'static str,
    f: fn() -> std::result::Result<std::path::PathBuf, paths::PathError>,
) -> Check {
    match f() {
        Ok(p) if p.is_dir() => Check::ok(label, p.display().to_string()),
        Ok(p) => Check::ok(label, format!("{} (will be created on first use)", p.display())),
        Err(e) => Check::fail(label, format!("could not resolve: {e}")),
    }
}

async fn check_db() -> Check {
    match paths::db_path() {
        Ok(p) if p.exists() => {
            match agentum_store::Store::open(&p).await {
                Ok(store) => {
                    let n = store
                        .list_sessions(None)
                        .await
                        .map(|v| v.len())
                        .unwrap_or(0);
                    Check::ok("database", format!("db.sqlite ({n} session{})", if n == 1 { "" } else { "s" }))
                }
                Err(e) => Check::fail("database", format!("exists but failed to open: {e}")),
            }
        }
        Ok(p) => Check::ok("database", format!("{} (will be created on first use)", p.display())),
        Err(e) => Check::fail("database", format!("could not resolve path: {e}")),
    }
}

fn check_tls() -> Check {
    match paths::tls_dir() {
        Ok(d) => {
            let cert = d.join("cert.pem");
            if cert.exists() {
                Check::ok("tls cert", cert.display().to_string())
            } else {
                Check::ok("tls cert", "not yet generated (created on first `agentum serve`)")
            }
        }
        Err(e) => Check::fail("tls cert", format!("could not resolve path: {e}")),
    }
}

async fn check_users() -> Check {
    match paths::db_path() {
        Ok(p) if p.exists() => match agentum_store::Store::open(&p).await {
            Ok(store) => match store.count_users().await {
                Ok(0) => Check::ok("users", "0 (register on first dashboard visit)"),
                Ok(n) => Check::ok("users", format!("{n} registered")),
                Err(e) => Check::fail("users", format!("query failed: {e}")),
            },
            Err(e) => Check::fail("users", format!("could not open db: {e}")),
        },
        Ok(_) => Check::ok("users", "(db not yet created)"),
        Err(e) => Check::fail("users", format!("could not resolve db path: {e}")),
    }
}

fn check_port(port: u16) -> Check {
    let label = "port 8822";
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => Check::ok(label, "available"),
        Err(_) => Check::ok(label, "in use (agentum serve may already be running)"),
    }
}
