use std::fs;
use std::path::PathBuf;

use agentum_store::paths;
use anyhow::{Context, Result, bail};

use crate::cli::ConfigCmd;

fn config_path() -> Result<PathBuf> {
    let dir = paths::config_dir().context("could not resolve config directory")?;
    Ok(dir.join("config.toml"))
}

fn read_doc(path: &PathBuf) -> Result<toml_edit::DocumentMut> {
    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        raw.parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("could not parse {}", path.display()))
    } else {
        Ok(toml_edit::DocumentMut::new())
    }
}

pub async fn run(action: ConfigCmd) -> Result<()> {
    match action {
        ConfigCmd::Get { key } => cmd_get(&key).await,
        ConfigCmd::Set { key, value } => cmd_set(&key, &value).await,
        ConfigCmd::Edit => cmd_edit().await,
    }
}

async fn cmd_get(key: &str) -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        bail!("no config file yet (create one with `agentum config set <key> <value>`)");
    }
    let doc = read_doc(&path)?;
    match doc.get(key) {
        Some(item) => {
            // Print the value without TOML decoration (strip quotes from strings).
            let display = match item.as_str() {
                Some(s) => s.to_string(),
                None => item.to_string().trim().to_string(),
            };
            println!("{display}");
            Ok(())
        }
        None => {
            eprintln!("key not found: {key}");
            std::process::exit(1);
        }
    }
}

async fn cmd_set(key: &str, value: &str) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut doc = read_doc(&path)?;

    // Try to parse as bool or integer, otherwise store as string.
    if value == "true" || value == "false" {
        doc[key] = toml_edit::value(value == "true");
    } else if let Ok(n) = value.parse::<i64>() {
        doc[key] = toml_edit::value(n);
    } else {
        doc[key] = toml_edit::value(value);
    }

    fs::write(&path, doc.to_string())
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("{key} = {value}");
    eprintln!("(written to {})", path.display());
    Ok(())
}

async fn cmd_edit() -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Touch the file so the editor opens something even if it doesn't exist yet.
    if !path.exists() {
        fs::write(&path, "# agentum configuration\n# see: agentum config set <key> <value>\n")?;
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = tokio::process::Command::new(&editor)
        .arg(&path)
        .status()
        .await
        .with_context(|| format!("could not launch editor: {editor}"))?;

    if !status.success() {
        bail!("editor exited with status {}", status.code().unwrap_or(-1));
    }
    Ok(())
}
