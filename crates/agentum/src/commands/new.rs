use std::path::PathBuf;
use std::process::Command;

use agentum_core::NewSession;
use anyhow::{Context, Result, bail};

use crate::cli::arg_to_flag;
use crate::commands::terminal::{YOLO_FLAG, YOLO_TOOLS};

pub async fn run(
    name: String,
    tool: String,
    dir: Option<PathBuf>,
    pick: bool,
    model: Option<String>,
    args: Vec<String>,
    up: bool,
    yolo: bool,
) -> Result<()> {
    let dir = resolve_workdir(dir, pick)?;

    if !dir.exists() {
        bail!("workdir does not exist: {}", dir.display());
    }
    let workdir = dir
        .canonicalize()
        .with_context(|| format!("could not canonicalize {}", dir.display()))?
        .to_string_lossy()
        .into_owned();

    let mut flags: Vec<String> = args.iter().map(|a| arg_to_flag(a)).collect();
    if yolo && YOLO_TOOLS.contains(&tool.as_str()) && !flags.iter().any(|f| f == YOLO_FLAG) {
        flags.push(YOLO_FLAG.to_string());
    }

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

fn resolve_workdir(dir: Option<PathBuf>, pick: bool) -> Result<PathBuf> {
    if let Some(d) = dir {
        return Ok(d);
    }
    if pick {
        return pick_with_lf();
    }
    std::env::current_dir().context("could not read current working directory")
}

fn pick_with_lf() -> Result<PathBuf> {
    if which::which("lf").is_err() {
        bail!(
            "`lf` is not installed or not on PATH. Install it from \
             https://github.com/gokcehan/lf and try again, or pass --dir <path>."
        );
    }

    let tmp = tempfile::NamedTempFile::new().context("could not create temp file for lf")?;
    let tmp_path = tmp.path().to_path_buf();

    let status = Command::new("lf")
        .arg("-last-dir-path")
        .arg(&tmp_path)
        .status()
        .context("failed to spawn lf")?;

    if !status.success() {
        bail!("lf exited with status {}", status);
    }

    let picked = std::fs::read_to_string(&tmp_path)
        .context("could not read lf's last-dir output")?
        .trim()
        .to_string();

    if picked.is_empty() {
        bail!("no directory was picked");
    }

    Ok(PathBuf::from(picked))
}
