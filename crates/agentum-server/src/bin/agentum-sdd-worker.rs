use std::path::PathBuf;

use agentum_server::sdd::remote_worker::{RemoteWorkerConfig, serve_stdio};

fn config_argument(arguments: &[String]) -> Result<PathBuf, String> {
    match arguments {
        [] => agentum_store::paths::config_dir()
            .map(|directory| directory.join("sdd-worker.json"))
            .map_err(|error| error.to_string()),
        [flag, path] if flag == "--config" => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("--config must be an absolute path".into());
            }
            Ok(path)
        }
        _ => Err("expected no arguments or --config <absolute-path>".into()),
    }
}

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.as_slice() {
        [flag] if flag == "--version" => {
            println!("agentum-sdd-worker {} protocol=1", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command, rest @ ..] if command == "--check-config" => match config_argument(rest) {
            Ok(path) => RemoteWorkerConfig::load(&path).map(|config| {
                println!(
                    "configuration valid: host={} repositories={}",
                    config.host_id,
                    config.repositories.len()
                );
            }),
            Err(error) => Err(
                agentum_server::sdd::remote_worker::RemoteWorkerError::Config(error),
            ),
        },
        [command, rest @ ..] if command == "subsystem" => match config_argument(rest) {
            Ok(path) => serve_stdio(&path).await,
            Err(error) => Err(
                agentum_server::sdd::remote_worker::RemoteWorkerError::Config(error),
            ),
        },
        [] => match config_argument(&[]) {
            Ok(path) => serve_stdio(&path).await,
            Err(error) => Err(
                agentum_server::sdd::remote_worker::RemoteWorkerError::Config(error),
            ),
        },
        _ => Err(
            agentum_server::sdd::remote_worker::RemoteWorkerError::Config(
                "usage: agentum-sdd-worker [subsystem] [--config <absolute-path>] | --check-config [--config <absolute-path>] | --version".into(),
            ),
        ),
    };
    if let Err(error) = result {
        // Never print configuration contents, repository paths, provider
        // output, request payloads, or credentials on the SSH diagnostic
        // channel. The stable category is sufficient for operators.
        eprintln!("agentum-sdd-worker failed: {}", error_category(&error));
        std::process::exit(1);
    }
}

fn error_category(error: &agentum_server::sdd::remote_worker::RemoteWorkerError) -> &'static str {
    use agentum_server::sdd::remote_worker::RemoteWorkerError;
    match error {
        RemoteWorkerError::Config(_) => "configuration",
        RemoteWorkerError::Store(_) => "durable-state",
        RemoteWorkerError::Workspace(_) => "workspace",
        RemoteWorkerError::Artifact(_) => "artifact",
        RemoteWorkerError::Provider(_) => "provider",
        RemoteWorkerError::Lifecycle(_) => "lifecycle",
        RemoteWorkerError::Invalid(_) => "request",
        RemoteWorkerError::BrowserVerificationUnavailable(_) => "browser_verification",
        RemoteWorkerError::Io(_) => "io",
        RemoteWorkerError::Json(_) => "protocol",
        RemoteWorkerError::Time(_) => "time",
        RemoteWorkerError::Path(_) => "paths",
    }
}
