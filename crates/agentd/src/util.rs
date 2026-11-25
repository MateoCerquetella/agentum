//! Small helpers shared by the modules.
use agentum_proto::Ms;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub fn now_ms() -> Ms {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/x` → `$HOME/x`.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// The first executable called `name` on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| {
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    })
}

/// Output of a finished command.
pub struct Out {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run a command to completion with a timeout (it is killed when the timeout hits).
pub async fn run(cmd: &mut Command, timeout: Duration) -> anyhow::Result<Out> {
    cmd.kill_on_drop(true).stdin(std::process::Stdio::null());
    let fut = cmd.output();
    let o = tokio::time::timeout(timeout, fut).await.map_err(|_| anyhow::anyhow!("timed out after {}s", timeout.as_secs()))??;
    Ok(Out {
        ok: o.status.success(),
        code: o.status.code(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    })
}

/// Cut `s` to at most `max` bytes on a char boundary, marking the cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes)", &s[..end], s.len() - end)
}

/// A random hex string of `n` bytes (from /dev/urandom; falls back to time and pid).
pub fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        let seed = now_ms() as u64 ^ ((std::process::id() as u64) << 32);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (seed.rotate_left(i as u32 * 7) & 0xff) as u8;
        }
    }
    hex::encode(buf)
}

/// Is `p` inside `root` (both already absolute and normalized)?
pub fn within(p: &Path, root: &Path) -> bool {
    p.starts_with(root)
}

/// The version number in a `--version` line ("2.1.3 (Claude Code)", "codex-cli 0.17.0", "v2.4.1").
pub fn parse_version(s: &str) -> Option<String> {
    s.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert_eq!(parse_version("2.1.323 (Claude Code)").as_deref(), Some("2.1.323"));
        assert_eq!(parse_version("codex-cli 0.172.0").as_deref(), Some("0.172.0"));
        assert_eq!(parse_version("v2.4.1").as_deref(), Some("2.4.1"));
        assert_eq!(parse_version("nothing here"), None);
    }
    #[test]
    fn cut() {
        assert_eq!(truncate("abc", 5), "abc");
        assert!(truncate("héllo wörld", 2).starts_with('h'));
    }
}
