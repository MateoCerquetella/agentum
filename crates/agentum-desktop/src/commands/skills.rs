use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

// Best-effort one-line scan of a SKILL.md YAML frontmatter `description:` value.
// No YAML dependency — the canonical agent skills carry a single-line
// description, which is all the Skills page renders.
fn read_skill_description(skill_file: &Path) -> Option<String> {
    let content = fs::read_to_string(skill_file).ok()?;
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("description:") {
            let value = rest
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn file_modified_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
}

fn count_dir_entries(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().count() as u64)
        .unwrap_or(0)
}

// Scan a single global agent-skill root (e.g. ~/.claude/skills): every direct
// subdirectory that contains a SKILL.md is one installed `home` skill. The
// renderer's hasInstalledAgentSkill matches on the skill name OR the directory
// basename, so name = directory basename is sufficient for detection.
fn discover_home_skills(skills_root: &Path) -> Vec<Value> {
    let mut skills: Vec<Value> = Vec::new();
    let entries = match fs::read_dir(skills_root) {
        Ok(entries) => entries,
        Err(_) => return skills,
    };
    let root_path = skills_root.to_string_lossy().to_string();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let skill_file = dir.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }
        let name = match dir.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        skills.push(json!({
            "id": format!("home:{name}"),
            "name": name,
            "description": read_skill_description(&skill_file),
            "providers": ["claude"],
            "sourceKind": "home",
            "sourceLabel": "Home (~/.claude/skills)",
            "rootPath": root_path,
            "directoryPath": dir.to_string_lossy(),
            "skillFilePath": skill_file.to_string_lossy(),
            "installed": true,
            "fileCount": count_dir_entries(&dir),
            "updatedAt": file_modified_ms(&skill_file),
        }));
    }
    skills.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    skills
}

// Discovery for the Skills page + the orchestration/CLI "Installed" probes.
// Scans the global home skills directory (`~/.claude/skills`) that
// `npx skills add --global` writes to. Repo/bundled/plugin sources aren't
// surfaced here yet; the renderer only requires the `home` source for its
// installed-skill checks (GLOBAL_AGENT_SKILL_SOURCE_KINDS = ['home']).
#[tauri::command]
pub fn skills_discover() -> Value {
    let scanned_at = now_ms();
    let home = match dirs::home_dir() {
        Some(home) => home,
        None => return json!({ "skills": [], "sources": [], "scannedAt": scanned_at }),
    };
    let skills_root = home.join(".claude").join("skills");
    let root_path = skills_root.to_string_lossy().to_string();
    let root_exists = skills_root.is_dir();
    let skills = discover_home_skills(&skills_root);

    let sources = json!([{
        "id": "home",
        "label": "Home (~/.claude/skills)",
        "path": root_path,
        "sourceKind": "home",
        "providers": ["claude"],
        "exists": root_exists,
    }]);

    json!({ "skills": skills, "sources": sources, "scannedAt": scanned_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_root(tag: &str) -> std::path::PathBuf {
        // Why: process id keeps parallel test runs from colliding without a
        // clock/random source (both are banned in workspace tests).
        std::env::temp_dir().join(format!("agentum-skills-{}-{}", tag, std::process::id()))
    }

    #[test]
    fn discovers_installed_home_skill() {
        let root = unique_temp_root("installed");
        let skill_dir = root.join("orchestration");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: orchestration\ndescription: \"Coordinate agents\"\n---\nbody\n",
        )
        .unwrap();

        let skills = discover_home_skills(&root);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "orchestration");
        assert_eq!(skills[0]["sourceKind"], "home");
        assert_eq!(skills[0]["installed"], true);
        assert_eq!(skills[0]["description"], "Coordinate agents");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ignores_dirs_without_skill_file_and_missing_roots() {
        let root = unique_temp_root("partial");
        fs::create_dir_all(root.join("not-a-skill")).unwrap();
        let skills = discover_home_skills(&root);
        assert!(skills.is_empty());
        fs::remove_dir_all(&root).ok();

        assert!(discover_home_skills(Path::new("/no/such/agentum/skills/root")).is_empty());
    }
}
