//! Harness gate helpers: prompt builders (feature / role / QA), verdict-file
//! paths + parsers, and small output/string utilities. Used by the drive loop
//! and gate runners; `pub(crate)` so those (in the parent module) can call them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::{Feature, RoleKind, SpecPhase};

pub(crate) fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    format!("{out}{err}")
}

/// Keep only the last `max` chars of `s` (so a huge verify log doesn't blow up
/// the stored error or the retry prompt we type into the pane).
pub(crate) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    // Snap to a char boundary so we never slice mid-UTF8.
    let start = (start..s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(s.len());
    format!("…\n{}", &s[start..])
}

/// Build the prompt handed to the agent for one feature: the harness
/// instructions (AGENTS.md) + the scoped feature + the gate contract.
pub(crate) fn build_feature_prompt(instructions: &str, feature: &Feature) -> String {
    if let Some(p) = &feature.prompt {
        return p.clone();
    }
    format!(
        "You are an agent running inside the Agentum Harness Engine.\n\n\
         === HARNESS INSTRUCTIONS (AGENTS.md) ===\n{instructions}\n\n\
         === YOUR CURRENT TASK — EXACTLY ONE FEATURE ===\n\
         Feature: {name}\n\
         ID: {id}\n\
         {desc}\n\n\
         Work ONLY on this feature. When you believe it is complete, stop and \
         wait. The harness will then run the verification gate (verify.sh). If \
         verification fails you will be given the error output and must fix it.",
        instructions = instructions.trim(),
        name = feature.name,
        id = feature.id,
        desc = feature.description,
    )
}

/// The verdict a role-agent writes after its gate turn (spec 013). Same
/// deterministic-file shape as [`QaVerdict`]: the harness reads a file instead of
/// parsing free-form chat. Written to `.agentum-harness/roles/<phase>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RoleVerdict {
    pub passed: bool,
    #[serde(default)]
    pub summary: Option<String>,
}

/// Where a role-agent writes its verdict, relative to the harness dir:
/// `roles/<phase>.json`.
pub(crate) fn role_verdict_path(harness_dir: &Path, phase: SpecPhase) -> PathBuf {
    harness_dir
        .join("roles")
        .join(format!("{}.json", phase.slug()))
}

/// Parse a role verdict file into `(passed, summary)`. Pure for testability. A
/// malformed/missing verdict is an error the caller turns into a failed gate —
/// an inconclusive role gate must NOT pass (mirrors [`parse_qa_verdict`] and the
/// harness's "agent self-report is never sufficient" rule).
pub(crate) fn parse_role_verdict(json: &str) -> anyhow::Result<(bool, String)> {
    let v: RoleVerdict = serde_json::from_str(json.trim())
        .map_err(|e| anyhow::anyhow!("role verdict is not valid JSON ({e}): {json}"))?;
    Ok((v.passed, v.summary.unwrap_or_default()))
}

/// Build the prompt for a role-agent gate (spec 013): the embedded role brief +
/// the harness instructions + the spec context + the exact verdict-file
/// contract. The "write this exact file" instruction is what makes the gate
/// deterministic and keeps the run fully autonomous (no chat-parsing, no human).
pub(crate) fn build_role_prompt(
    role: RoleKind,
    instructions: &str,
    spec_id: &str,
    spec_md: &str,
    verdict_rel_path: &str,
) -> String {
    format!(
        "{brief}\n\n\
         === HARNESS INSTRUCTIONS (AGENTS.md) ===\n{instructions}\n\n\
         === SPEC UNDER REVIEW: {spec_id} ===\n{spec}\n\n\
         === HOW TO RECORD YOUR VERDICT ===\n\
         When finished, WRITE your verdict to `{verdict}` (relative to the project \
         root) as exactly this JSON:\n\
         {{\"passed\": true|false, \"summary\": \"one line on what passed or the single most important gap\"}}\n\
         Set passed=false if the gate does not pass. Do not stop until the file is \
         written. Do not ask the human anything.",
        brief = role.brief().trim(),
        instructions = instructions.trim(),
        spec_id = spec_id,
        spec = spec_md.trim(),
        verdict = verdict_rel_path,
    )
}

/// The machine-readable verdict the QA agent writes (spec 012b). The agent runs
/// the browser-verification-loop, then writes this file so the harness has a
/// deterministic pass/fail instead of trying to parse free-form chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct QaVerdict {
    pub passed: bool,
    #[serde(default)]
    pub summary: Option<String>,
}

/// Where the QA agent writes its verdict, relative to the harness dir:
/// `qa/<feature_id>.json`. The feature id is sanitized so it's a safe filename.
pub(crate) fn qa_verdict_path(harness_dir: &Path, feature_id: &str) -> PathBuf {
    harness_dir
        .join("qa")
        .join(format!("{}.json", sanitize(feature_id)))
}

/// Parse a QA verdict file into `(passed, summary)`. Pure for testability. A
/// malformed/missing verdict is an error the caller turns into a red gate — an
/// inconclusive QA must NOT pass (we'd mark a feature Done without evidence).
pub(crate) fn parse_qa_verdict(json: &str) -> anyhow::Result<(bool, String)> {
    let v: QaVerdict = serde_json::from_str(json.trim())
        .map_err(|e| anyhow::anyhow!("QA verdict is not valid JSON ({e}): {json}"))?;
    Ok((v.passed, v.summary.unwrap_or_default()))
}

/// Build the prompt for the QA agent: run the browser-verification-loop for this
/// one feature, then write the verdict file. The explicit "write this exact file"
/// contract is what makes the gate deterministic.
pub(crate) fn build_qa_prompt(
    instructions: &str,
    feature: &Feature,
    verdict_rel_path: &str,
) -> String {
    format!(
        "You are the QA agent in the Agentum Harness Engine. The implementation of \
         ONE feature just passed its unit-test gate; your job is to verify it in a \
         REAL browser and record a verdict.\n\n\
         === HARNESS INSTRUCTIONS (AGENTS.md) ===\n{instructions}\n\n\
         === FEATURE UNDER TEST ===\n\
         Feature: {name}\nID: {id}\n{desc}\n\n\
         === WHAT TO DO ===\n\
         1. Use the `browser-verification-loop` skill (Chrome/Playwright MCP) to QA \
         this feature against the running app. Capture a screenshot per check as evidence.\n\
         2. When finished, WRITE your verdict to `{verdict}` (relative to the project \
         root) as exactly this JSON:\n\
         {{\"passed\": true|false, \"summary\": \"one line on what you verified or why it failed\"}}\n\
         Set passed=false if ANY check fails or you cannot verify. Do not stop until \
         the file is written.",
        instructions = instructions.trim(),
        name = feature.name,
        id = feature.id,
        desc = feature.description,
        verdict = verdict_rel_path,
    )
}

/// Make a string safe to embed in a tmux session name.
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
