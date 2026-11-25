//! Files for the hub: list, read, write (with a conflict check) and fuzzy find.
//! Paths must be absolute and inside the user's home or a project root.
use agentum_proto::machine::FsReq;
use anyhow::{Context, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

pub const MAX_READ: u64 = 2 * 1024 * 1024;

/// Where file requests may go.
#[derive(Clone, Debug)]
pub struct Allowed {
    pub roots: Vec<PathBuf>,
}

impl Allowed {
    /// The real path of `p`, if it is allowed. A path that doesn't exist yet is checked by its parent.
    pub fn check(&self, p: &str) -> anyhow::Result<PathBuf> {
        let path = Path::new(p);
        if !path.is_absolute() {
            bail!("{p}: not an absolute path");
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!("{p}: no .. please");
        }
        let real = match path.canonicalize() {
            Ok(r) => r,
            Err(_) => {
                let parent = path.parent().context("no parent")?.canonicalize().with_context(|| format!("{}: no such directory", path.parent().unwrap().display()))?;
                parent.join(path.file_name().context("no file name")?)
            }
        };
        if self.roots.iter().any(|r| real.starts_with(r)) {
            Ok(real)
        } else {
            bail!("{p}: outside your home and projects")
        }
    }
}

pub fn sha_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn looks_binary(b: &[u8]) -> bool {
    b[..b.len().min(8192)].contains(&0) || std::str::from_utf8(b).is_err()
}

/// Blocking: run on a blocking thread.
pub fn handle(req: FsReq, allowed: &Allowed) -> anyhow::Result<Value> {
    match req {
        FsReq::List { path } => {
            let p = allowed.check(&path)?;
            let mut entries: Vec<(bool, String, u64, bool)> = Vec::new();
            for e in std::fs::read_dir(&p).with_context(|| format!("listing {}", p.display()))? {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                // Follow symlinks for the kind, but keep a broken one listed as a file.
                let md = std::fs::metadata(e.path()).or_else(|_| e.metadata());
                let (dir, size) = md.map(|m| (m.is_dir(), if m.is_dir() { 0 } else { m.len() })).unwrap_or((false, 0));
                let hidden = name.starts_with('.');
                entries.push((dir, name, size, hidden));
            }
            entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
            Ok(json!({
                "path": p,
                "entries": entries.into_iter().map(|(dir, name, size, hidden)| json!({"name": name, "dir": dir, "size": size, "hidden": hidden})).collect::<Vec<_>>(),
            }))
        }
        FsReq::Read { path } => {
            let p = allowed.check(&path)?;
            let md = std::fs::metadata(&p).with_context(|| format!("{}: not found", p.display()))?;
            if md.is_dir() {
                bail!("{}: is a directory", p.display());
            }
            if md.len() > MAX_READ {
                bail!("{}: too big ({} bytes; the limit is 2 MB)", p.display(), md.len());
            }
            let b = std::fs::read(&p)?;
            if looks_binary(&b) {
                bail!("{}: binary file", p.display());
            }
            let sha = sha_hex(&b);
            Ok(json!({"path": p, "content": String::from_utf8(b).unwrap_or_default(), "sha": sha, "size": md.len()}))
        }
        FsReq::Write { path, content, expect_sha } => {
            let p = allowed.check(&path)?;
            if let Some(want) = expect_sha {
                let have = std::fs::read(&p).ok().map(|b| sha_hex(&b));
                if have.as_deref() != Some(want.as_str()) {
                    bail!("conflict: {} changed on disk", p.display());
                }
            }
            let tmp = p.with_file_name(format!(".{}.agentd-{}", p.file_name().unwrap().to_string_lossy(), crate::util::random_hex(4)));
            std::fs::write(&tmp, content.as_bytes())?;
            // Keep the file's mode.
            if let Ok(md) = std::fs::metadata(&p) {
                let _ = std::fs::set_permissions(&tmp, md.permissions());
            }
            if let Err(e) = std::fs::rename(&tmp, &p) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
            Ok(json!({"path": p, "sha": sha_hex(content.as_bytes()), "size": content.len()}))
        }
        FsReq::Find { root, query, limit } => {
            let r = allowed.check(&root)?;
            Ok(json!({"root": r, "paths": find(&r, &query, limit.unwrap_or(50) as usize)}))
        }
    }
}

const SKIP: [&str; 6] = [".git", "node_modules", "target", ".venv", "__pycache__", ".next"];

/// Subsequence match of `q` in `name` (case-insensitive): a score, higher is better.
pub fn fuzzy(name: &str, q: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    let mut i = 0;
    for qc in q.to_lowercase().chars() {
        let pos = n[i..].iter().position(|&c| c == qc)? + i;
        score += 10;
        match last {
            Some(l) if pos == l + 1 => score += 15,
            _ => {}
        }
        if pos == 0 || matches!(n.get(pos.wrapping_sub(1)), Some('/' | '_' | '-' | '.' | ' ')) {
            score += 8;
        }
        last = Some(pos);
        i = pos + 1;
    }
    Some(score - n.len() as i64 / 4)
}

pub fn find(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let q = query.trim();
    let mut hits: Vec<(i64, String)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > 200_000 {
                break;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if !SKIP.contains(&name.as_str()) {
                    stack.push(e.path());
                }
                continue;
            }
            let rel = e.path().strip_prefix(root).map(|p| p.to_string_lossy().into_owned()).unwrap_or(name.clone());
            // The file name counts most; the path breaks ties.
            if let Some(s) = fuzzy(&name, q).map(|s| s + 20).or_else(|| fuzzy(&rel, q)) {
                hits.push((s, rel));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())).then_with(|| a.1.cmp(&b.1)));
    hits.into_iter().take(limit).map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentd-fs-{}", crate::util::random_hex(4)));
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn list_read_write_find() {
        let d = tmp();
        let a = Allowed { roots: vec![d.clone()] };
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::write(d.join("node_modules/x/main.rs"), "").unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(d.join(".env"), "A=1").unwrap();
        std::fs::write(d.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let ls = handle(FsReq::List { path: d.to_string_lossy().into() }, &a).unwrap();
        let names: Vec<&str> = ls["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["node_modules", "src", ".env", "bin.dat"]);
        assert_eq!(ls["entries"][2]["hidden"], true);
        let p = d.join("src/main.rs").to_string_lossy().into_owned();
        let r = handle(FsReq::Read { path: p.clone() }, &a).unwrap();
        assert_eq!(r["content"], "fn main() {}\n");
        let sha = r["sha"].as_str().unwrap().to_string();
        assert_eq!(sha.len(), 64);
        assert!(handle(FsReq::Read { path: d.join("bin.dat").to_string_lossy().into() }, &a).unwrap_err().to_string().contains("binary"));
        let w = handle(FsReq::Write { path: p.clone(), content: "x".into(), expect_sha: Some(sha.clone()) }, &a).unwrap();
        assert_eq!(w["sha"], sha_hex(b"x"));
        // The file changed since `sha`: refuse.
        let e = handle(FsReq::Write { path: p.clone(), content: "y".into(), expect_sha: Some(sha) }, &a).unwrap_err();
        assert!(e.to_string().contains("conflict"));
        handle(FsReq::Write { path: d.join("new.txt").to_string_lossy().into(), content: "n".into(), expect_sha: None }, &a).unwrap();
        let f = handle(FsReq::Find { root: d.to_string_lossy().into(), query: "mainrs".into(), limit: None }, &a).unwrap();
        assert_eq!(f["paths"], json!(["src/main.rs"]));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn refuses_outside_and_relative() {
        let d = tmp();
        let a = Allowed { roots: vec![d.clone()] };
        assert!(a.check("relative/x").is_err());
        assert!(a.check("/etc/passwd").is_err());
        assert!(a.check(&format!("{}/../x", d.display())).is_err());
        assert!(a.check(&format!("{}/not-yet.txt", d.display())).is_ok());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn fuzzy_prefers_tight_matches() {
        assert!(fuzzy("main.rs", "main").unwrap() > fuzzy("domain.rs", "main").unwrap());
        assert!(fuzzy("abc", "abd").is_none());
    }
}
