use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const INSTALL_URL: &str =
    "https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh";

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Server,
    Cli,
}

impl Mode {
    fn flag(self) -> &'static str {
        match self {
            Mode::Server => "server",
            Mode::Cli => "cli",
        }
    }
}

pub async fn run(mode: Option<Mode>, force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    eprintln!("agentum update — current version: v{current}");

    let downloader = pick_downloader().context(
        "neither `curl` nor `wget` is available on PATH; install one to update agentum",
    )?;

    let pipe = which("sh").context("`sh` is required to run the installer")?;
    let _ = pipe; // presence check only

    let installer = fetch_installer(&downloader)?;

    let mut sh = Command::new("sh");
    sh.arg("-s").arg("--");
    if let Some(m) = mode {
        sh.arg("--mode").arg(m.flag());
    } else {
        // Re-run non-interactively when stdin is not a TTY (e.g. piped runs).
        // The installer also auto-detects, but be explicit.
        if !is_stdin_tty() {
            sh.arg("--no-interactive");
        }
    }
    if force {
        sh.env("AGENTUM_FORCE_UPDATE", "1");
    }
    sh.stdin(Stdio::piped());
    sh.stdout(Stdio::inherit());
    sh.stderr(Stdio::inherit());

    let mut child = sh.spawn().context("failed to spawn `sh`")?;
    {
        use std::io::Write as _;
        let mut stdin = child
            .stdin
            .take()
            .context("failed to open stdin for `sh`")?;
        stdin
            .write_all(installer.as_bytes())
            .context("failed to pipe installer to `sh`")?;
    }
    let status = child.wait().context("installer process failed to run")?;
    if !status.success() {
        bail!("installer exited with status {status}");
    }
    Ok(())
}

fn pick_downloader() -> Option<Downloader> {
    if which("curl").is_some() {
        Some(Downloader::Curl)
    } else if which("wget").is_some() {
        Some(Downloader::Wget)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum Downloader {
    Curl,
    Wget,
}

fn fetch_installer(d: &Downloader) -> Result<String> {
    let out = match d {
        Downloader::Curl => Command::new("curl")
            .args(["-fsSL", INSTALL_URL])
            .output()
            .context("failed to invoke curl")?,
        Downloader::Wget => Command::new("wget")
            .args(["-qO-", INSTALL_URL])
            .output()
            .context("failed to invoke wget")?,
    };
    if !out.status.success() {
        bail!(
            "downloading installer failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("installer is not valid UTF-8")
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_stdin_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}
