use std::path::PathBuf;

use agentum_core::NewSession;
use anyhow::{Context, Result, bail};

use crate::cli::arg_to_flag;

pub async fn run(
    name: String,
    tool: String,
    dir: PathBuf,
    model: Option<String>,
    args: Vec<String>,
    up: bool,
) -> Result<()> {
    if !dir.exists() {
        bail!("workdir does not exist: {}", dir.display());
    }
    let workdir = dir
        .canonicalize()
        .with_context(|| format!("could not canonicalize {}", dir.display()))?
        .to_string_lossy()
        .into_owned();

    let flags: Vec<String> = args.iter().map(|a| arg_to_flag(a)).collect();

    let (store, _) = super::open_store().await?;
    let session = store
        .create_session(NewSession {
            name: name.clone(),
            workdir,
            tool: tool.clone(),
            model,
            flags,
        })
        .await?;

    println!(
        "registered  {name}  ({tool})  id={id}",
        name = session.name,
        tool = session.tool,
        id = session.id,
    );

    if up {
        super::up::run(name).await?;
    }
    Ok(())
}
