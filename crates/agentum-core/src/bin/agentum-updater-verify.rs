use agentum_core::updater_signature::verify_tauri_updater_signature;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let arguments = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if arguments.len() != 3 {
        bail!("usage: agentum-updater-verify <tauri.conf.json> <artifact> <signature>");
    }
    let config: Value = serde_json::from_slice(
        &fs::read(&arguments[0]).with_context(|| format!("read {}", arguments[0].display()))?,
    )
    .context("parse Tauri configuration")?;
    let public_key = config
        .pointer("/plugins/updater/pubkey")
        .and_then(Value::as_str)
        .context("Tauri configuration has no updater public key")?;
    let artifact =
        fs::read(&arguments[1]).with_context(|| format!("read {}", arguments[1].display()))?;
    let signature = fs::read_to_string(&arguments[2])
        .with_context(|| format!("read {}", arguments[2].display()))?;
    verify_tauri_updater_signature(public_key, &signature, &artifact)
        .with_context(|| format!("verify {}", arguments[1].display()))?;
    println!("verified updater signature: {}", arguments[1].display());
    Ok(())
}
