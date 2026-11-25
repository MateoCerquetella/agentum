//! Git: status, diffs, commit and push for the thread's checkout (a tab in its side panel), and
//! a header badge with the uncommitted change ("+12 −3 · 4 files").
//!
//! The hub part answers `stat {thread}` and, after every turn, publishes the thread's new stat
//! on `plugin:git` (`{thread, stat}`), so the badge follows what the agent did.
use agentum_plugin::{Group, Manifest};
use serde::{Deserialize, Serialize};

pub static MANIFEST: Manifest = Manifest {
    id: "git",
    name: "Git",
    description: "Status, diffs, commit and push for the thread's checkout.",
    icon: "branch",
    group: Group::Code,
    default_on: true,
    settings: &[],
};

/// A checkout's uncommitted change and where its branch stands.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Stat {
    pub add: u32,
    pub del: u32,
    pub files: u32,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
}

/// Lines added and removed, and files, in a `git diff` (hunk counts tell content from headers,
/// so a removed line "-- x" isn't read as a "--- " header).
pub fn numstat(diff: &str) -> Stat {
    let mut s = Stat::default();
    let (mut old, mut new) = (0u32, 0u32);
    for l in diff.lines() {
        if old > 0 || new > 0 {
            match l.as_bytes().first() {
                Some(b'+') => {
                    s.add += 1;
                    new = new.saturating_sub(1);
                    continue;
                }
                Some(b'-') => {
                    s.del += 1;
                    old = old.saturating_sub(1);
                    continue;
                }
                Some(b'\\') => continue,
                Some(b' ') | None => {
                    old = old.saturating_sub(1);
                    new = new.saturating_sub(1);
                    continue;
                }
                _ => (old, new) = (0, 0),
            }
        }
        if l.starts_with("diff --git ") {
            s.files += 1;
        } else if let Some(r) = l.strip_prefix("@@ -") {
            let count = |x: &str| x.split_once(',').map(|(_, n)| n.parse().unwrap_or(0)).unwrap_or(1);
            let mut it = r.split(' ');
            old = it.next().map(count).unwrap_or(0);
            new = it.next().and_then(|x| x.strip_prefix('+')).map(count).unwrap_or(0);
        }
    }
    s
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;

#[cfg(test)]
mod tests {
    #[test]
    fn counts_lines_and_files() {
        let d = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n x\n-- not a header\n+y\n z\ndiff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+1\n+2\n\\ No newline at end of file\n";
        let s = super::numstat(d);
        assert_eq!((s.add, s.del, s.files), (3, 1, 2));
        assert_eq!(super::numstat(""), super::Stat::default());
    }
}
