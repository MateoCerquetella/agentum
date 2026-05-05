use std::net::TcpListener;

use agentum_store::paths;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;
use tokio::process::Command;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/doctor", get(doctor))
}

#[derive(Serialize)]
pub struct Check {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Serialize)]
struct DoctorReport {
    ok: bool,
    failures: u32,
    checks: Vec<Check>,
}

async fn doctor(State(state): State<AppState>) -> Json<DoctorReport> {
    let checks = vec![
        check_tmux().await,
        check_dir("data dir", paths::data_dir),
        check_dir("config dir", paths::config_dir),
        check_db(&state).await,
        check_tls(),
        check_users(&state).await,
        check_port(8822),
    ];

    let failures = checks.iter().filter(|c| !c.passed).count() as u32;
    Json(DoctorReport {
        ok: failures == 0,
        failures,
        checks,
    })
}

async fn check_tmux() -> Check {
    match Command::new("tmux").arg("-V").output().await {
        Ok(out) if out.status.success() => Check {
            label: "tmux".into(),
            passed: true,
            detail: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        },
        Ok(_) => Check {
            label: "tmux".into(),
            passed: false,
            detail: "tmux found but returned an error".into(),
        },
        Err(_) => Check {
            label: "tmux".into(),
            passed: false,
            detail: "not found — install with your package manager".into(),
        },
    }
}

fn check_dir(
    label: &'static str,
    f: fn() -> std::result::Result<std::path::PathBuf, paths::PathError>,
) -> Check {
    match f() {
        Ok(p) if p.is_dir() => Check {
            label: label.into(),
            passed: true,
            detail: p.display().to_string(),
        },
        Ok(p) => Check {
            label: label.into(),
            passed: true,
            detail: format!("{} (will be created on first use)", p.display()),
        },
        Err(e) => Check {
            label: label.into(),
            passed: false,
            detail: format!("could not resolve: {e}"),
        },
    }
}

async fn check_db(state: &AppState) -> Check {
    let n = state.store.list_sessions(None).await.map(|v| v.len()).unwrap_or(0);
    let detail = match paths::db_path() {
        Ok(p) => format!("{} ({n} session{})", p.display(), if n == 1 { "" } else { "s" }),
        Err(_) => format!("({n} session{})", if n == 1 { "" } else { "s" }),
    };
    Check { label: "database".into(), passed: true, detail }
}

fn check_tls() -> Check {
    match paths::tls_dir() {
        Ok(d) => {
            let cert = d.join("cert.pem");
            if cert.exists() {
                Check {
                    label: "tls cert".into(),
                    passed: true,
                    detail: cert.display().to_string(),
                }
            } else {
                Check {
                    label: "tls cert".into(),
                    passed: true,
                    detail: "not yet generated".into(),
                }
            }
        }
        Err(e) => Check {
            label: "tls cert".into(),
            passed: false,
            detail: format!("could not resolve path: {e}"),
        },
    }
}

async fn check_users(state: &AppState) -> Check {
    match state.store.count_users().await {
        Ok(0) => Check {
            label: "users".into(),
            passed: true,
            detail: "0 (register on first dashboard visit)".into(),
        },
        Ok(n) => Check {
            label: "users".into(),
            passed: true,
            detail: format!("{n} registered"),
        },
        Err(e) => Check {
            label: "users".into(),
            passed: false,
            detail: format!("query failed: {e}"),
        },
    }
}

fn check_port(port: u16) -> Check {
    let label = format!("port {port}");
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => Check { label, passed: true, detail: "available".into() },
        Err(_) => Check {
            label,
            passed: true,
            detail: "in use (agentum serve may already be running)".into(),
        },
    }
}
