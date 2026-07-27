use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentum_server::sdd::credentials::{
    OsCredentialVault, SddCredentialVault, headless_vault_or_unavailable,
};
use agentum_server::sdd::provider_conformance::{
    publish_report, run_bundled_suite, run_custom_suite, verify_checkpoint_file, verify_report_file,
};

fn value(arguments: &[String], name: &str) -> Result<String, String> {
    let indexes: Vec<_> = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| (argument == name).then_some(index))
        .collect();
    if indexes.len() != 1 {
        return Err(format!("{name} must be supplied exactly once"));
    }
    arguments
        .get(indexes[0] + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn repeated_values(arguments: &[String], name: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == name {
            let next = arguments
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{name} requires a value"))?;
            values.push(next.clone());
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(values)
}

fn absolute_path(value: String, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute path"));
    }
    Ok(path)
}

fn reject_unknown(arguments: &[String], allowed: &[&str]) -> Result<(), String> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if !allowed.contains(&argument.as_str()) {
            return Err(format!("unknown argument: {argument}"));
        }
        if arguments.get(index + 1).is_none() {
            return Err(format!("{argument} requires a value"));
        }
        index += 2;
    }
    Ok(())
}

fn custom_vault() -> Arc<dyn SddCredentialVault> {
    if std::env::var_os("AGENTUM_SDD_VAULT_MASTER_KEY").is_some() {
        headless_vault_or_unavailable()
    } else {
        Arc::new(OsCredentialVault::new())
    }
}

fn require_existing_directory(path: &Path, name: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| format!("{name} does not exist or cannot be inspected"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{name} must be a real directory"));
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = dispatch(&arguments).await;
    if let Err(error) = result {
        eprintln!("agentum provider conformance failed: {error}");
        std::process::exit(1);
    }
}

async fn dispatch(arguments: &[String]) -> Result<(), String> {
    let Some((command, rest)) = arguments.split_first() else {
        return Err(usage().into());
    };
    match command.as_str() {
        "--version" if rest.is_empty() => {
            println!(
                "agentum-sdd-provider-conformance {} suite={}",
                env!("CARGO_PKG_VERSION"),
                agentum_server::sdd::providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE
            );
            Ok(())
        }
        "run-bundled" => {
            reject_unknown(rest, &["--source-revision", "--output", "--provider"])?;
            let source_revision = value(rest, "--source-revision")?;
            let output = absolute_path(value(rest, "--output")?, "--output")?;
            let providers = repeated_values(rest, "--provider")?;
            let bundle = run_bundled_suite(&providers, &source_revision)
                .await
                .map_err(|error| error.to_string())?;
            publish_report(&output, &bundle).map_err(|error| error.to_string())?;
            println!(
                "provider conformance passed: providers={} source={}",
                bundle.reports.len(),
                bundle.source_revision
            );
            Ok(())
        }
        "run-custom" => {
            reject_unknown(
                rest,
                &["--source-revision", "--output", "--provider-dir", "--id"],
            )?;
            let source_revision = value(rest, "--source-revision")?;
            let output = absolute_path(value(rest, "--output")?, "--output")?;
            let directory = absolute_path(value(rest, "--provider-dir")?, "--provider-dir")?;
            require_existing_directory(&directory, "--provider-dir")?;
            let id = value(rest, "--id")?;
            let vault = custom_vault();
            if !vault.status().available {
                return Err("secure credential vault is unavailable".into());
            }
            let bundle = run_custom_suite(&directory, &id, &source_revision, vault.as_ref())
                .await
                .map_err(|error| error.to_string())?;
            publish_report(&output, &bundle).map_err(|error| error.to_string())?;
            println!("custom provider conformance passed: id={id}");
            Ok(())
        }
        "verify-report" => {
            reject_unknown(
                rest,
                &["--report", "--source-revision", "--require-provider"],
            )?;
            let report = absolute_path(value(rest, "--report")?, "--report")?;
            let source_revision = value(rest, "--source-revision")?;
            let required = repeated_values(rest, "--require-provider")?;
            if required.is_empty() {
                return Err("--require-provider must be supplied at least once".into());
            }
            verify_report_file(&report, &source_revision, &required)
                .map_err(|error| error.to_string())?;
            println!("provider conformance report verified");
            Ok(())
        }
        "verify-checkpoint" => {
            reject_unknown(rest, &["--checkpoint", "--expected-hash"])?;
            let checkpoint = absolute_path(value(rest, "--checkpoint")?, "--checkpoint")?;
            let expected_hash = value(rest, "--expected-hash")?;
            verify_checkpoint_file(&checkpoint, &expected_hash).map_err(|error| error.to_string())
        }
        _ => Err(usage().into()),
    }
}

fn usage() -> &'static str {
    "usage: agentum-sdd-provider-conformance run-bundled --source-revision <revision> --output <absolute-path> [--provider <id> ...] | run-custom --provider-dir <absolute-path> --id <id> --source-revision <revision> --output <absolute-path> | verify-report --report <absolute-path> --source-revision <revision> --require-provider <id> [...]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cli_rejects_relative_and_unknown_arguments_without_running_a_provider() {
        assert!(
            dispatch(&[
                "run-bundled".into(),
                "--source-revision".into(),
                "abc".into(),
                "--output".into(),
                "relative.json".into(),
            ])
            .await
            .is_err()
        );
        assert!(
            dispatch(&[
                "verify-report".into(),
                "--report".into(),
                "/tmp/report.json".into(),
                "--source-revision".into(),
                "abc".into(),
                "--surprise".into(),
                "value".into(),
            ])
            .await
            .is_err()
        );
    }
}
