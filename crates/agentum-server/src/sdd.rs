//! Server-owned SDD playbooks — the `/sdd-*` workflow as an app capability.
//!
//! These procedures used to live as untracked `.claude/commands/*.md` files on
//! one developer machine: invisible to a fresh install and Claude-only. The
//! server embeds them (`include_str!`, same pattern as `harness_roles/`) and is
//! the single source of truth for every consumer: the MCP surface
//! (`prompts/list`/`prompts/get` + the `agentum_sdd` tool in `routes::mcp`),
//! the desktop SDD buttons and per-session SDD loop (`routes::sdd`), and —
//! per spec 013's contract — future harness role phases. Playbooks are
//! *delivered* to agents, never copied into user repos.
//!
//! A per-user override lives at `~/.agentum/commands/<name>.md`
//! (`$AGENTUM_HOME/commands` when set), so self-hosters can tune a playbook
//! without rebuilding.

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

/// One SDD playbook: an agent-facing procedure the server owns.
#[derive(Debug, Clone, Serialize)]
pub struct Playbook {
    /// Canonical id, e.g. `sdd-spec` — also the MCP prompt name.
    pub name: String,
    /// Human label for UI surfaces ("Spec", "Spec Socratic", …).
    pub title: String,
    /// One-line summary, parsed from the playbook's frontmatter.
    pub description: String,
    /// The full procedure body, frontmatter stripped.
    pub body: String,
}

/// `(name, title, embedded default)` for the shipped playbooks. The files are
/// verbatim copies of the original `.claude/commands/sdd-*.md`; keep them in
/// sync by editing HERE (this is the canonical home now, not `~/.claude`).
const EMBEDDED: &[(&str, &str, &str)] = &[
    (
        "sdd-spec",
        "Spec",
        include_str!("sdd_playbooks/sdd-spec.md"),
    ),
    (
        "sdd-spec-socratic",
        "Spec Socratic",
        include_str!("sdd_playbooks/sdd-spec-socratic.md"),
    ),
    (
        "sdd-orchestrate",
        "Orchestrate",
        include_str!("sdd_playbooks/sdd-orchestrate.md"),
    ),
    (
        "sdd-status",
        "Status",
        include_str!("sdd_playbooks/sdd-status.md"),
    ),
    (
        "sdd-handoff",
        "Handoff",
        include_str!("sdd_playbooks/sdd-handoff.md"),
    ),
    (
        "sdd-init",
        "Init",
        include_str!("sdd_playbooks/sdd-init.md"),
    ),
];

/// Where per-user playbook overrides live. `$AGENTUM_HOME` wins so tests (and
/// portable installs) can isolate without touching the real `$HOME/.agentum`.
fn override_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("AGENTUM_HOME") {
        return Some(PathBuf::from(home).join("commands"));
    }
    agentum_core::home_dir().map(|h| h.join(".agentum").join("commands"))
}

/// The full registry, override-aware. Cheap (six small strings + at most six
/// `stat` calls) — callers are low-traffic HTTP/MCP endpoints, so no cache.
pub fn playbooks() -> Vec<Playbook> {
    playbooks_from(override_dir().as_deref())
}

/// Look up one playbook by canonical name (`sdd-spec`, …).
pub fn get(name: &str) -> Option<Playbook> {
    playbooks().into_iter().find(|p| p.name == name)
}

/// Registry against an explicit override dir — the testable seam (same
/// pattern as `mcp_provision`: no process-global env mutation in tests).
fn playbooks_from(dir: Option<&Path>) -> Vec<Playbook> {
    EMBEDDED
        .iter()
        .map(|(name, title, embedded)| {
            let raw = dir
                .map(|d| d.join(format!("{name}.md")))
                .filter(|p| p.is_file())
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_else(|| (*embedded).to_string());
            let (description, body) = split_frontmatter(&raw);
            Playbook {
                name: (*name).to_string(),
                title: (*title).to_string(),
                description,
                body,
            }
        })
        .collect()
}

/// Split a `---\ndescription: …\n---` frontmatter header off the body. A file
/// without frontmatter is all body with an empty description — overrides
/// shouldn't have to know the convention to work.
fn split_frontmatter(raw: &str) -> (String, String) {
    // Git checks text files out with CRLF by default on Windows. Normalize the
    // embedded/override input so the same frontmatter grammar works on every
    // release target.
    let normalized = raw.replace("\r\n", "\n");
    let raw = normalized.as_str();
    let Some(rest) = raw.strip_prefix("---\n") else {
        return (String::new(), raw.trim().to_string());
    };
    let Some((header, body)) = rest.split_once("\n---") else {
        return (String::new(), raw.trim().to_string());
    };
    let description = header
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    (description, body.trim().to_string())
}

/// The short prompt a button/loop injects into the pane. Deliberately NOT the
/// playbook body: the agent fetches that itself over MCP, so the injection is
/// tool-agnostic, the input box stays small, and a playbook update lands
/// without re-injecting anything.
pub fn bootstrap_prompt(playbook: &Playbook, args: Option<&str>) -> String {
    let mut prompt = format!(
        "Call the `agentum_sdd` tool on the agentum MCP server with {{\"name\": \"{}\"}}, \
         then follow the returned playbook exactly. ({}.) If the agentum MCP server is \
         not available, say so instead of improvising the procedure.",
        playbook.name, playbook.description
    );
    if let Some(args) = args.map(str::trim).filter(|a| !a.is_empty()) {
        prompt.push_str(&format!(" Arguments for the playbook: {args}"));
    }
    prompt
}

/// Fallback for sessions whose tool has no MCP wiring (plain shells, aider…):
/// deliver the whole playbook as the prompt instead of the bootstrap line.
pub fn full_prompt(playbook: &Playbook, args: Option<&str>) -> String {
    let mut prompt = format!(
        "Follow this playbook exactly.\n\n# {} — {}\n\n{}",
        playbook.name, playbook.description, playbook.body
    );
    if let Some(args) = args.map(str::trim).filter(|a| !a.is_empty()) {
        prompt.push_str(&format!("\n\nArguments: {args}"));
    }
    prompt
}

/// One step of the automated SDD loop, wrapped around a base prompt (the
/// bootstrap line or, for unwired tools, the full playbook). The check-in
/// instruction matters — the loop's server side only sees settle events, so
/// the `agentum_sdd_loop` call ending the turn is the loop's only way to stop
/// the moment the work is done instead of re-injecting to the step cap. The
/// session id and generation are embedded because the MCP layer has no
/// ambient caller identity (same explicit-id pattern as
/// `agentum_report_status`).
pub fn loop_step_prompt(step: u32, session_id: Uuid, generation: u64, base_prompt: &str) -> String {
    format!(
        "SDD loop step {step} (automated — no human is watching this pane). {base_prompt} \
         END this step by calling the `agentum_sdd_loop` tool on the agentum MCP server \
         with exactly {{\"session\": \"{session_id}\", \"generation\": {generation}, \
         \"done\": true|false, \"summary\": \"<one line>\"}} — `done: true` when \
         `ai/STATE.md` says the current spec's phase is `done` or there is no actionable \
         next step (and then do NOT start new work), `done: false` otherwise. If you \
         cannot call MCP tools, reply with that done verdict instead and stop."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ships_six_playbooks_with_bodies_and_descriptions() {
        let all = playbooks_from(None);
        assert_eq!(all.len(), 6);
        for p in &all {
            assert!(p.name.starts_with("sdd-"), "canonical names: {}", p.name);
            assert!(!p.description.is_empty(), "{} has a description", p.name);
            assert!(!p.body.is_empty(), "{} has a body", p.name);
            // Frontmatter must be stripped — agents get the procedure, not headers.
            assert!(!p.body.starts_with("---"), "{} body is header-free", p.name);
        }
        let names: std::collections::HashSet<_> = all.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), all.len(), "names are unique");
    }

    #[test]
    fn frontmatter_accepts_windows_line_endings() {
        let (description, body) =
            split_frontmatter("---\r\ndescription: Windows-safe\r\n---\r\n\r\nDo it.\r\n");
        assert_eq!(description, "Windows-safe");
        assert_eq!(body, "Do it.");
    }

    #[test]
    fn override_file_replaces_embedded_body() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("sdd-spec.md"),
            "---\ndescription: my custom spec flow\n---\n\nDo it my way.",
        )
        .unwrap();
        let all = playbooks_from(Some(dir.path()));
        let spec = all.iter().find(|p| p.name == "sdd-spec").unwrap();
        assert_eq!(spec.description, "my custom spec flow");
        assert_eq!(spec.body, "Do it my way.");
        // Others still come from the embedded defaults.
        let status = all.iter().find(|p| p.name == "sdd-status").unwrap();
        assert!(status.body.contains("STATE.md"));
    }

    #[test]
    fn bootstrap_prompt_names_the_tool_and_playbook_not_the_body() {
        let p = get("sdd-orchestrate").unwrap();
        let prompt = bootstrap_prompt(&p, Some("autonomous"));
        assert!(prompt.contains("agentum_sdd"));
        assert!(prompt.contains("sdd-orchestrate"));
        assert!(prompt.contains("autonomous"));
        // The point of the bootstrap: the body travels over MCP, not send-keys.
        assert!(!prompt.contains("validate_handoff"));
    }

    #[test]
    fn full_prompt_carries_the_body_for_unwired_tools() {
        let p = get("sdd-status").unwrap();
        assert!(full_prompt(&p, None).contains("Suggest next action"));
    }

    #[test]
    fn loop_step_prompt_embeds_session_id_and_checkin_instruction() {
        let p = get("sdd-orchestrate").unwrap();
        let id = Uuid::new_v4();
        let prompt = loop_step_prompt(3, id, 7, &bootstrap_prompt(&p, Some("autonomous")));
        assert!(prompt.contains("step 3"));
        assert!(prompt.contains("autonomous"));
        // The check-in must be addressable without ambient identity: the tool
        // call names the session and the activation it came from.
        assert!(prompt.contains("agentum_sdd_loop"));
        assert!(prompt.contains(&id.to_string()));
        assert!(prompt.contains("\"generation\": 7"));
        assert!(prompt.contains("done"));
        // The unread completion sentence is gone — the tool call replaced it.
        assert!(!prompt.contains("reply briefly"));
    }
}
