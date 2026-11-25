//! `agentd` / `agentd run` · `agentd install --hub URL --token T --name N [--ca PATH]` ·
//! `agentd status` · `agentd version`
//!
//! At start it reads `$AGENTD_ENV_FILE` (default `~/.config/agentum/agentd.env`) into its
//! environment, without overriding what is set; `AGENTD_*` variables override agentd.toml.
use agentd::config::Config;
use agentd::install::{InstallArgs, install, status};

const USAGE: &str = "agentd: Agentum's machine daemon

usage:
  agentd [run]            run in the foreground (what the service does)
  agentd install --hub URL --token TOKEN --name NAME [--ca CA.pem]
                          write ~/.config/agentum/agentd.toml and a systemd unit, and start it
  agentd status           is it running and connected?
  agentd version

environment (also read from $AGENTD_ENV_FILE, default ~/.config/agentum/agentd.env):
  AGENTD_HUB AGENTD_TOKEN AGENTD_NAME AGENTD_CA AGENTD_HUB_IP AGENTD_ROOTS AGENTD_PROJECTS
                          override agentd.toml (lists separated by : or ,)
  AGENTD_CONFIG           agentd.toml's path
";

fn flag(args: &[String], name: &str) -> Option<String> {
    let eq = format!("{name}=");
    args.iter().enumerate().find_map(|(i, a)| if a == name { args.get(i + 1).cloned() } else { a.strip_prefix(&eq).map(str::to_string) })
}

fn main() {
    // Before anything else (and any thread): variables from agentd.env, under those already set.
    let env_file = Config::env_file();
    // SAFETY: no other thread exists yet.
    let env = unsafe { agentum_proto::dotenv::load(&env_file) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("run");
    let r = match cmd {
        "run" => run(&env_file, env),
        "install" => {
            let need = |n: &str| flag(&args, n).ok_or_else(|| anyhow::anyhow!("missing {n}\n\n{USAGE}"));
            (|| install(InstallArgs { hub: need("--hub")?, token: need("--token")?, name: need("--name")?, ca: flag(&args, "--ca") }))()
        }
        "status" => status(),
        "version" | "--version" | "-V" => {
            println!("agentd {} (protocol {})", agentd::VERSION, agentum_proto::PROTOCOL);
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown command {other:?}\n\n{USAGE}")),
    };
    if let Err(e) = r {
        eprintln!("agentd: {e:#}");
        std::process::exit(1);
    }
}

fn run(env_file: &std::path::Path, env: std::io::Result<Vec<String>>) -> anyhow::Result<()> {
    let level = match std::env::var("AGENTD_LOG").ok().as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("warn") => tracing::Level::WARN,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    tracing_subscriber::fmt().with_max_level(level).with_target(false).with_writer(std::io::stderr).init();
    match env {
        Ok(keys) if !keys.is_empty() => tracing::info!("{}: {}", env_file.display(), keys.join(" ")),
        Ok(_) => {}
        Err(e) => tracing::warn!("{}: {e}", env_file.display()),
    }
    let cfg = Config::load()?;
    // The token stays out of the agents' and terminals' environment.
    // SAFETY: still no other thread (the runtime starts below).
    unsafe { std::env::remove_var("AGENTD_TOKEN") };
    // A small runtime: agentd mostly waits.
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).max_blocking_threads(16).enable_all().build()?;
    rt.block_on(async move {
        let app = agentd::App::new(cfg).await?;
        tracing::info!("agentd {} for {} ({})", agentd::VERSION, app.cfg.hub, app.cfg.name);
        app.run().await
    })
}
