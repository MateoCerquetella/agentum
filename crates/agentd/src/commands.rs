//! The composer's `/` menu: an agent's slash commands and skills, read from where each agent keeps
//! them (the project's, then the user's), and their expansion for agents that don't run them
//! from a prompt themselves.
//!
//! - Claude Code: `.claude/commands/**.md` (a subdirectory is a `dir:` prefix), `.claude/skills/*/SKILL.md`,
//!   the enabled plugins' `commands/` and `skills/` (`plugin:name`), and a few built-ins. It runs
//!   them itself: the text goes as typed.
//! - Codex: `~/.codex/prompts/*.md`, skills in `.codex/skills` and `.agents/skills`.
//! - OpenCode: `.opencode/command(s)/*.md` (`~/.config/opencode/…`), skills in `.opencode/skill(s)`
//!   and `.claude/skills`.
//! - Cursor: `.cursor/commands/*.md`, skills in `.cursor/skills`.
//!
//! For those three agentd expands `/name args` before the agent sees it: a command becomes its
//! file's body (`$ARGUMENTS`, `$1`…`$9` filled in), a skill a line telling the agent to read its
//! SKILL.md and follow it.
use agentum_proto::AgentKind;
use agentum_proto::machine::{CommandInfo, CommandKind};
use std::path::{Path, PathBuf};

/// A command or skill and the file it comes from.
#[derive(Clone, Debug)]
pub struct Found {
    pub info: CommandInfo,
    pub path: PathBuf,
}

const MAX: usize = 400;
const MAX_FILE: u64 = 256 * 1024;

fn claude_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"))
}
fn codex_dir(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"))
}

/// What `agent` offers in `cwd`, the project's first; one entry per name (the first found wins).
pub fn list(agent: AgentKind, cwd: &Path, home: &Path) -> Vec<Found> {
    let mut out: Vec<Found> = vec![];
    let cmds = |out: &mut Vec<Found>, dir: PathBuf, origin: &str, prefix: &str| scan_commands(out, &dir, &dir, origin, prefix);
    let skills = |out: &mut Vec<Found>, dir: PathBuf, origin: &str, prefix: &str| scan_skills(out, &dir, origin, prefix);
    match agent {
        AgentKind::ClaudeCode => {
            let cd = claude_dir(home);
            cmds(&mut out, cwd.join(".claude/commands"), "project", "");
            skills(&mut out, cwd.join(".claude/skills"), "project", "");
            cmds(&mut out, cd.join("commands"), "user", "");
            skills(&mut out, cd.join("skills"), "user", "");
            for (name, root) in claude_plugins(&cd) {
                let p = format!("{name}:");
                cmds(&mut out, root.join("commands"), "plugin", &p);
                skills(&mut out, root.join("skills"), "plugin", &p);
            }
            for (name, d) in [
                ("compact", "Summarize the conversation to free up context"),
                ("init", "Write a CLAUDE.md that describes this project"),
                ("security-review", "Review the pending changes for security issues"),
            ] {
                push(&mut out, Found { info: CommandInfo { name: name.into(), kind: CommandKind::Builtin, origin: "builtin".into(), description: Some(d.into()), hint: None }, path: PathBuf::new() });
            }
        }
        AgentKind::Codex => {
            let cd = codex_dir(home);
            skills(&mut out, cwd.join(".codex/skills"), "project", "");
            skills(&mut out, cwd.join(".agents/skills"), "project", "");
            cmds(&mut out, cd.join("prompts"), "user", "");
            skills(&mut out, cd.join("skills"), "user", "");
            skills(&mut out, home.join(".agents/skills"), "user", "");
        }
        AgentKind::Opencode => {
            let cd = home.join(".config/opencode");
            for d in ["command", "commands"] {
                cmds(&mut out, cwd.join(".opencode").join(d), "project", "");
            }
            for d in [".opencode/skill", ".opencode/skills", ".claude/skills"] {
                skills(&mut out, cwd.join(d), "project", "");
            }
            for d in ["command", "commands"] {
                cmds(&mut out, cd.join(d), "user", "");
            }
            for d in [cd.join("skill"), cd.join("skills"), claude_dir(home).join("skills")] {
                skills(&mut out, d, "user", "");
            }
        }
        AgentKind::Cursor => {
            cmds(&mut out, cwd.join(".cursor/commands"), "project", "");
            skills(&mut out, cwd.join(".cursor/skills"), "project", "");
            cmds(&mut out, home.join(".cursor/commands"), "user", "");
            skills(&mut out, home.join(".cursor/skills"), "user", "");
        }
    }
    out
}

fn push(out: &mut Vec<Found>, f: Found) {
    if out.len() < MAX && !out.iter().any(|x| x.info.name == f.info.name) {
        out.push(f);
    }
}

fn sorted_entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    let mut v: Vec<_> = std::fs::read_dir(dir).map(|r| r.flatten().collect()).unwrap_or_default();
    v.sort_by_key(|e| e.file_name());
    v
}

fn read_small(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    if m.len() > MAX_FILE {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// `<dir>/**/*.md` (three levels), a subdirectory a `sub:` prefix.
fn scan_commands(out: &mut Vec<Found>, root: &Path, dir: &Path, origin: &str, prefix: &str) {
    let depth = dir.strip_prefix(root).map(|r| r.components().count()).unwrap_or(0);
    for e in sorted_entries(dir) {
        let p = e.path();
        let fname = e.file_name().to_string_lossy().to_string();
        if fname.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if depth < 3 {
                scan_commands(out, root, &p, origin, prefix);
            }
            continue;
        }
        let Some(stem) = fname.strip_suffix(".md") else { continue };
        let Some(text) = read_small(&p) else { continue };
        let rel: Vec<String> = p.parent().and_then(|d| d.strip_prefix(root).ok()).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect()).unwrap_or_default();
        let mut name = prefix.to_string();
        for r in rel {
            name += &r;
            name.push(':');
        }
        name += stem;
        let (fm, body) = front_matter(&text);
        let description = fm_get(fm, "description").or_else(|| first_line(body));
        push(out, Found { info: CommandInfo { name, kind: CommandKind::Command, origin: origin.into(), description, hint: fm_get(fm, "argument-hint") }, path: p });
    }
}

/// `<dir>/<skill>/SKILL.md`, named after `<skill>`.
fn scan_skills(out: &mut Vec<Found>, dir: &Path, origin: &str, prefix: &str) {
    for e in sorted_entries(dir) {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path().join("SKILL.md");
        let Some(text) = read_small(&p) else { continue };
        let (fm, body) = front_matter(&text);
        // Its directory's name, as Claude Code calls it (a front matter `name` may differ: "Poteto Mode").
        let name = e.file_name().to_string_lossy().to_string();
        let description = fm_get(fm, "description").or_else(|| first_line(body));
        push(out, Found { info: CommandInfo { name: format!("{prefix}{name}"), kind: CommandKind::Skill, origin: origin.into(), description, hint: None }, path: p });
    }
}

/// Claude Code's enabled plugins: (name, install directory).
fn claude_plugins(cd: &Path) -> Vec<(String, PathBuf)> {
    let read = |p: PathBuf| read_small(&p).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).unwrap_or_default();
    let enabled = read(cd.join("settings.json"));
    let enabled = enabled.get("enabledPlugins").and_then(|e| e.as_object());
    let installed = read(cd.join("plugins/installed_plugins.json"));
    let mut v = vec![];
    for (key, installs) in installed.get("plugins").and_then(|p| p.as_object()).into_iter().flatten() {
        if !enabled.and_then(|e| e.get(key)).and_then(|b| b.as_bool()).unwrap_or(false) {
            continue;
        }
        let name = key.split('@').next().unwrap_or(key).to_string();
        let path = installs.as_array().and_then(|a| a.first()).or(Some(installs)).and_then(|i| i.get("installPath")).and_then(|p| p.as_str());
        if let Some(p) = path {
            v.push((name, PathBuf::from(p)));
        }
    }
    v.sort();
    v
}

/// `---\n…\n---` at the top: (front matter, the rest).
fn front_matter(text: &str) -> (&str, &str) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(rest) = t.strip_prefix("---\n").or_else(|| t.strip_prefix("---\r\n")) {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            let after = after.split_once('\n').map(|(_, b)| b).unwrap_or("");
            return (&rest[..end], after);
        }
    }
    ("", t)
}

/// One key of a YAML front matter: `key: value`, quoted, or a `>`/`|` block.
fn fm_get(fm: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = fm.lines().collect();
    let i = lines.iter().position(|l| l.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with(':')))?;
    let v = lines[i][key.len()..].trim_start()[1..].trim();
    let out = if v.is_empty() || v.starts_with('>') || v.starts_with('|') {
        let block: Vec<&str> = lines[i + 1..].iter().take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.is_empty()).map(|l| l.trim()).collect();
        block.join(" ").trim().to_string()
    } else {
        v.trim_matches(|c| c == '"' || c == '\'').to_string()
    };
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty()).then_some(out)
}

fn first_line(body: &str) -> Option<String> {
    let l = body.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty())?;
    Some(if l.chars().count() > 120 { format!("{}…", l.chars().take(119).collect::<String>()) } else { l.to_string() })
}

/// What the agent should get for `text`, when it is `/name args` and `agent` can't run it itself
/// (None: send `text` as it is).
pub fn expand(agent: AgentKind, cwd: &Path, home: &Path, text: &str) -> Option<String> {
    if agent == AgentKind::ClaudeCode {
        return None;
    }
    let rest = text.trim_start().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).map(|(n, a)| (n, a.trim())).unwrap_or((rest.trim_end(), ""));
    if name.is_empty() {
        return None;
    }
    let f = list(agent, cwd, home).into_iter().find(|f| f.info.name == name)?;
    match f.info.kind {
        CommandKind::Command => {
            let body = read_small(&f.path)?;
            let (_, body) = front_matter(&body);
            Some(fill(body.trim(), args))
        }
        CommandKind::Skill => {
            let mut s = format!("Use the \"{name}\" skill: read {} and follow its instructions.", f.path.display());
            if !args.is_empty() {
                s += "\n\n";
                s += args;
            }
            Some(s)
        }
        CommandKind::Builtin => None,
    }
}

/// A command's body with its arguments: `$ARGUMENTS`, `$1`…`$9`; appended when it names none.
fn fill(body: &str, args: &str) -> String {
    let words: Vec<&str> = args.split_whitespace().collect();
    let mut used = body.contains("$ARGUMENTS");
    let mut s = body.replace("$ARGUMENTS", args);
    for i in (1..=9).rev() {
        let k = format!("${i}");
        if s.contains(&k) {
            used = true;
            s = s.replace(&k, words.get(i - 1).copied().unwrap_or(""));
        }
    }
    if !used && !args.is_empty() {
        s += "\n\n";
        s += args;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentd-cmds-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn write(p: PathBuf, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn reads_front_matter() {
        let fm = "name: ponytail\ndescription: >\n  Lazy senior\n  dev mode.\nargument-hint: \"[file]\"";
        assert_eq!(fm_get(fm, "description").as_deref(), Some("Lazy senior dev mode."));
        assert_eq!(fm_get(fm, "argument-hint").as_deref(), Some("[file]"));
        assert_eq!(fm_get(fm, "name").as_deref(), Some("ponytail"));
        assert_eq!(fm_get(fm, "missing"), None);
        let (fm, body) = front_matter("---\ndescription: x\n---\nBody $ARGUMENTS\n");
        assert_eq!((fm, body), ("description: x", "Body $ARGUMENTS\n"));
    }

    #[test]
    fn lists_and_expands() {
        let (home, cwd) = (tmp(), tmp());
        write(cwd.join(".opencode/command/fix.md"), "---\ndescription: Fix an issue\nargument-hint: [issue]\n---\nFix issue $1 now. All: $ARGUMENTS");
        write(home.join(".config/opencode/command/fix.md"), "the user's fix (shadowed)");
        write(home.join(".config/opencode/command/git/sync.md"), "# Sync the branch\nPull and rebase.");
        write(home.join(".claude/skills/tidy/SKILL.md"), "---\nname: tidy\ndescription: Tidy up\n---\nSteps");
        let l = list(AgentKind::Opencode, &cwd, &home);
        let names: Vec<_> = l.iter().map(|f| (f.info.name.as_str(), f.info.origin.as_str())).collect();
        assert_eq!(names, [("fix", "project"), ("git:sync", "user"), ("tidy", "user")]);
        assert_eq!(l[0].info.hint.as_deref(), Some("[issue]"));
        assert_eq!(l[1].info.description.as_deref(), Some("Sync the branch"));
        assert_eq!(expand(AgentKind::Opencode, &cwd, &home, "/fix 42 fast").as_deref(), Some("Fix issue 42 now. All: 42 fast"));
        assert_eq!(expand(AgentKind::Opencode, &cwd, &home, "/git:sync").as_deref(), Some("# Sync the branch\nPull and rebase."));
        let s = expand(AgentKind::Opencode, &cwd, &home, "/tidy src/").unwrap();
        assert!(s.starts_with("Use the \"tidy\" skill: read ") && s.ends_with("SKILL.md and follow its instructions.\n\nsrc/"), "{s}");
        assert_eq!(expand(AgentKind::Opencode, &cwd, &home, "/nope x"), None);
        assert_eq!(expand(AgentKind::Opencode, &cwd, &home, "not a command"), None);
        // Claude Code runs them itself.
        write(cwd.join(".claude/commands/fix.md"), "x");
        assert_eq!(expand(AgentKind::ClaudeCode, &cwd, &home, "/fix 1"), None);
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    fn fills_arguments() {
        assert_eq!(fill("Do it.", "now"), "Do it.\n\nnow");
        assert_eq!(fill("Do it.", ""), "Do it.");
        assert_eq!(fill("$2 then $1", "a b"), "b then a");
    }
}
