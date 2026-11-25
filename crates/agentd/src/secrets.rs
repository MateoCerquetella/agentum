//! A project's `.env`, for the Secrets tab: key names go out, values come in and go only into the
//! file (written atomically, mode 600). Nothing here logs, and no error carries a value.
use crate::fs::Allowed;
use agentum_proto::dotenv;
use agentum_proto::machine::SecretsReq;
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX: u64 = 1024 * 1024;

/// Blocking: run on a blocking thread.
pub fn handle(req: SecretsReq, allowed: &Allowed) -> anyhow::Result<Value> {
    let dir = match &req {
        SecretsReq::Keys { dir } | SecretsReq::Set { dir, .. } | SecretsReq::Delete { dir, .. } => dir,
    };
    let dir = allowed.check(dir)?;
    if !dir.is_dir() {
        bail!("{}: not a directory", dir.display());
    }
    let file = dir.join(".env");
    let mut added = false;
    match req {
        SecretsReq::Keys { .. } => {}
        SecretsReq::Set { key, value, .. } => {
            if !dotenv::valid_key(&key) {
                bail!("{key:?} isn't a variable name (letters, digits and _, not starting with a digit)");
            }
            let text = read(&file)?;
            write(&file, &dotenv::set(&text, &key, &value))?;
            added = ensure_ignored(&dir)?;
        }
        SecretsReq::Delete { key, .. } => {
            let text = read(&file)?;
            let new = dotenv::remove(&text, &key);
            if new != text {
                write(&file, &new)?;
            }
        }
    }
    let (ignored, tracked) = git_state(&dir);
    Ok(json!({
        "dir": dir,
        "keys": dotenv::keys(&read(&file)?),
        "exists": file.exists(),
        // null: not in a git repo.
        "ignored": ignored,
        "tracked": tracked,
        "gitignore_added": added,
    }))
}

fn read(file: &Path) -> anyhow::Result<String> {
    match std::fs::metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e).with_context(|| format!("{}", file.display())),
        Ok(m) if m.is_dir() => bail!("{}: is a directory", file.display()),
        Ok(m) if m.len() > MAX => bail!("{}: too big for a .env", file.display()),
        Ok(_) => {}
    }
    let b = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    String::from_utf8(b).map_err(|_| anyhow::anyhow!("{}: not text", file.display()))
}

/// Replace `file` with `text`: a new file (mode 600) beside it, synced, then renamed over it.
fn write(file: &Path, text: &str) -> anyhow::Result<()> {
    let dir = file.parent().context("no directory")?;
    let tmp = dir.join(format!(".env.agentum-{}.tmp", crate::util::random_hex(6)));
    let r = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, file)
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", file.display()));
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Option<bool> {
    Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().ok().map(|s| s.success())
}

/// (Does git ignore `.env`? None: not a repo; is it committed?)
fn git_state(dir: &Path) -> (Option<bool>, bool) {
    if git(dir, &["rev-parse", "--is-inside-work-tree"]) != Some(true) {
        return (None, false);
    }
    let ignored = git(dir, &["check-ignore", "-q", "--no-index", ".env"]) == Some(true);
    let tracked = git(dir, &["ls-files", "--error-unmatch", "--", ".env"]) == Some(true);
    (Some(ignored), tracked)
}

/// In a git repo that doesn't ignore `.env`: add it to `<dir>/.gitignore`. True if it did.
fn ensure_ignored(dir: &Path) -> anyhow::Result<bool> {
    if git_state(dir).0 != Some(false) {
        return Ok(false);
    }
    let gi = dir.join(".gitignore");
    let mut text = std::fs::read_to_string(&gi).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(".env\n");
    std::fs::write(&gi, text).with_context(|| format!("writing {}", gi.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn set_list_delete() {
        let root = std::env::temp_dir().join(format!("agentd-secrets-{}", crate::util::random_hex(4)));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let allowed = Allowed { roots: vec![root.clone()] };
        let d = root.to_string_lossy().to_string();
        assert!(Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap().success());
        std::fs::write(root.join(".gitignore"), "target/").unwrap();

        let r = handle(SecretsReq::Keys { dir: d.clone() }, &allowed).unwrap();
        assert_eq!(r["keys"], json!([]));
        assert_eq!(r["exists"], false);
        assert_eq!(r["ignored"], false);

        let r = handle(SecretsReq::Set { dir: d.clone(), key: "API_KEY".into(), value: "s3cret value".into() }, &allowed).unwrap();
        assert_eq!(r["keys"], json!(["API_KEY"]));
        assert_eq!(r["gitignore_added"], true);
        assert_eq!(r["ignored"], true);
        assert!(!r.to_string().contains("s3cret"));
        assert_eq!(std::fs::read_to_string(root.join(".gitignore")).unwrap(), "target/\n.env\n");
        assert_eq!(std::fs::metadata(root.join(".env")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(dotenv::parse(&std::fs::read_to_string(root.join(".env")).unwrap()), vec![("API_KEY".into(), "s3cret value".into())]);

        // A second key, an update: .gitignore is left alone now.
        handle(SecretsReq::Set { dir: d.clone(), key: "B".into(), value: "1".into() }, &allowed).unwrap();
        let r = handle(SecretsReq::Set { dir: d.clone(), key: "API_KEY".into(), value: "new".into() }, &allowed).unwrap();
        assert_eq!(r["keys"], json!(["API_KEY", "B"]));
        assert_eq!(r["gitignore_added"], false);
        assert_eq!(std::fs::read_to_string(root.join(".env")).unwrap(), "API_KEY=new\nB=1\n");

        let r = handle(SecretsReq::Delete { dir: d.clone(), key: "API_KEY".into() }, &allowed).unwrap();
        assert_eq!(r["keys"], json!(["B"]));

        assert!(handle(SecretsReq::Set { dir: d.clone(), key: "BAD KEY".into(), value: "x".into() }, &allowed).is_err());
        assert!(handle(SecretsReq::Keys { dir: "/etc".into() }, &allowed).is_err());
        // No temp files left behind.
        let left: Vec<String> = std::fs::read_dir(&root).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".tmp")).collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}
