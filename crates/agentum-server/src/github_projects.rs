//! GitHub Projects v2 board bindings (spec 010 F1).
//!
//! The domain module for the "gated loop lands on the user's real Projects v2
//! board" feature: a per-repo [`BoardBinding`] maps every canonical pipeline
//! phase to a single-select *option ID* on the project's Status field. This
//! module owns the binding types, the one-call `gh api graphql` Status-field
//! discovery, the pure fuzzy name→phase mapper, and the persistence file —
//! the `linear.rs` precedent (domain logic at crate root, routes stay thin,
//! `task_sink` calls in for F2).
//!
//! Persistence is a server-owned sibling file `github_projects.json` (010 D2 =
//! a2): Settings label saves round-trip `github.json` through a typed struct
//! that DROPS unknown keys, so bindings live in their own single-writer file —
//! clobber-immune by construction.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// serde default for [`BoardBinding::done_closes_issue`] — D1's wizard-default-ON
/// exists in exactly ONE definition site (§7.7): a binding persisted or fetched
/// without the knob reads ON, and callers building a binding from an absent wire
/// field use this same fn.
pub(crate) fn default_true() -> bool {
    true
}

/// The five-phase board vocabulary. LOCAL to the projects layer —
/// `TrackerPhase` stays four variants (008 D-A stands); Blocked exists only
/// here and on the GitHub-label layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardPhase {
    Todo,
    InProgress,
    ReadyToTest,
    Done,
    Blocked,
}

impl From<crate::task_sink::TrackerPhase> for BoardPhase {
    fn from(phase: crate::task_sink::TrackerPhase) -> Self {
        use crate::task_sink::TrackerPhase::*;
        match phase {
            Todo => BoardPhase::Todo,
            InProgress => BoardPhase::InProgress,
            ReadyToTest => BoardPhase::ReadyToTest,
            Done => BoardPhase::Done,
        }
    }
}

/// One single-select OPTION ID per canonical phase. Five REQUIRED `String`
/// fields — an unmapped phase is unrepresentable by type, and a stored file
/// missing any phase fails deserialization → reads as "no binding", so a
/// partial binding can never exist on disk either (AC 2d).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusMapping {
    pub todo: String,
    pub in_progress: String,
    pub ready_to_test: String,
    pub done: String,
    pub blocked: String,
}

impl StatusMapping {
    /// The stored option ID for a phase — the value every board write sends
    /// (IDs, never names: renames after bind still land).
    pub fn option_id(&self, phase: BoardPhase) -> &str {
        match phase {
            BoardPhase::Todo => &self.todo,
            BoardPhase::InProgress => &self.in_progress,
            BoardPhase::ReadyToTest => &self.ready_to_test,
            BoardPhase::Done => &self.done,
            BoardPhase::Blocked => &self.blocked,
        }
    }
}

/// The same five-field shape carrying option *names* — display/round-trip
/// metadata only (never used at write time). Every field defaults so a
/// partial/stale names blob can never brick an otherwise-valid binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusNames {
    #[serde(default)]
    pub todo: String,
    #[serde(default)]
    pub in_progress: String,
    #[serde(default)]
    pub ready_to_test: String,
    #[serde(default)]
    pub done: String,
    #[serde(default)]
    pub blocked: String,
}

/// A per-repo Projects v2 binding: which project, which Status field, and the
/// phase → option-ID mapping every transition writes with (spec 010 F2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardBinding {
    pub project_id: String,
    pub status_field_id: String,
    pub status_mapping: StatusMapping,
    /// D1 knob: a Done transition closes the issue / a later InProgress
    /// reopens it. THE default materializes here (serde default = true): a
    /// binding written without it reads ON; the UI renders the stored value
    /// and writes it explicitly — one definition site (§7.7).
    #[serde(default = "default_true")]
    pub done_closes_issue: bool,
    // Display/round-trip metadata (names are NEVER used at write time — IDs only):
    #[serde(default)]
    pub project_title: Option<String>,
    #[serde(default)]
    pub project_owner: Option<String>,
    /// `"user"` | `"organization"`.
    #[serde(default)]
    pub project_owner_type: Option<String>,
    #[serde(default)]
    pub project_number: Option<i64>,
    #[serde(default)]
    pub option_names: Option<StatusNames>,
}

// ─── Persistence: the sibling github_projects.json (D2 = a2) ───────────────

/// The persisted file shape. Key = lowercase `owner/repo` slug.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GithubProjectsFile {
    #[serde(default)]
    bindings: std::collections::BTreeMap<String, BoardBinding>,
}

/// Serializes read-modify-write cycles on the bindings file. Server-side twin
/// of the desktop `github_labels.rs` STORE_LOCK pattern; nothing else writes
/// `github_projects.json`, so this is the single writer by construction.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Path to the server-owned bindings file. Mirrors
/// `task_sink::github_config_path` exactly (`<data_local_dir|data_dir>/Agentum/
/// github_projects.json`); `AGENTUM_GITHUB_PROJECTS_CONFIG` overrides it
/// (tests/CI — mirrors `AGENTUM_GITHUB_CONFIG`).
fn github_projects_config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AGENTUM_GITHUB_PROJECTS_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = dirs::data_local_dir().or_else(dirs::data_dir)?;
    Some(base.join("Agentum").join("github_projects.json"))
}

/// Absent/unreadable/garbled → `Default` (no bindings), never an error — a
/// binding read must resolve even on a machine that never bound a board. A
/// garbled file also lands here, which is what makes a stored binding with a
/// missing phase read as "no binding" (AC 2d) rather than a partial one.
fn read_bindings_at(path: &Path) -> GithubProjectsFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Path-injected core of [`binding_for_slug`] — tests pass a temp file, never
/// the real config (the hermeticity discipline: no env mutation).
pub(crate) fn binding_for_slug_at(path: &Path, slug: &str) -> Option<BoardBinding> {
    read_bindings_at(path)
        .bindings
        .get(&slug.trim().to_lowercase())
        .cloned()
}

/// The binding for a repo slug (case-insensitive), FRESH from disk on every
/// call — the `GithubStateMap::from_env` freshness contract: a bind applies on
/// the next transition, no restart.
pub fn binding_for_slug(slug: &str) -> Option<BoardBinding> {
    binding_for_slug_at(&github_projects_config_path()?, slug)
}

fn write_bindings_at(path: &Path, file: &GithubProjectsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(file)
        .map_err(|e| format!("could not serialize bindings: {e}"))?;
    std::fs::write(path, raw).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Path-injected core of [`upsert_binding`]. WRITE_LOCK'd read-modify-write.
pub(crate) fn upsert_binding_at(
    path: &Path,
    slug: &str,
    binding: BoardBinding,
) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_bindings_at(path);
    file.bindings.insert(slug.trim().to_lowercase(), binding);
    write_bindings_at(path, &file)
}

/// Insert-or-replace the binding for a slug (lowercased key).
pub fn upsert_binding(slug: &str, binding: BoardBinding) -> Result<(), String> {
    let path = github_projects_config_path().ok_or("no config directory available")?;
    upsert_binding_at(&path, slug, binding)
}

/// Path-injected core of [`remove_binding`]. Returns whether a binding existed.
pub(crate) fn remove_binding_at(path: &Path, slug: &str) -> Result<bool, String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_bindings_at(path);
    let existed = file.bindings.remove(&slug.trim().to_lowercase()).is_some();
    if existed {
        write_bindings_at(path, &file)?;
    }
    Ok(existed)
}

/// Remove a slug's binding. `Ok(false)` when none existed (idempotent).
pub fn remove_binding(slug: &str) -> Result<bool, String> {
    let path = github_projects_config_path().ok_or("no config directory available")?;
    remove_binding_at(&path, slug)
}

// ─── The pure fuzzy mapper ──────────────────────────────────────────────────

/// Normalize a column name for matching: lowercase, keep `[a-z0-9]` only —
/// spaces, dashes, underscores, emoji, punctuation all stripped, so
/// `"🚧 In-Progress "` → `"inprogress"`.
fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

// The synonym tables (normalized tokens), disjoint by construction — pinned by
// `synonym_lists_are_disjoint`. Matching is exact-normalized ONLY, no substring
// ("notstarted" contains "started", "notdone" contains "done" — a false
// positive is worse than a miss; misses go to the fallbacks or D7's manual
// selects).
const TODO_SYNONYMS: &[&str] = &[
    "todo", "backlog", "new", "triage", "inbox", "upnext", "planned",
];
const IN_PROGRESS_SYNONYMS: &[&str] = &[
    "inprogress",
    "doing",
    "building",
    "wip",
    "started",
    "active",
    "development",
    "indevelopment",
    "coding",
];
const READY_TO_TEST_SYNONYMS: &[&str] = &[
    "readytotest",
    "readyfortest",
    "qa",
    "testing",
    "test",
    "review",
    "inreview",
    "readyforreview",
    "verify",
    "verification",
    "staging",
];
const DONE_SYNONYMS: &[&str] = &[
    "done",
    "shipped",
    "complete",
    "completed",
    "finished",
    "closed",
    "merged",
    "released",
];
const BLOCKED_SYNONYMS: &[&str] = &["blocked", "stuck", "onhold", "hold", "waiting", "paused"];

/// How a phase's option was resolved. `FellBack` ⇒ the UI renders the D5 hint
/// ("no 'Ready to Test'-like column — falls back to In Progress; add one and
/// re-discover").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchVia {
    Matched,
    FellBack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPhase {
    pub option_id: String,
    pub option_name: String,
    pub via: MatchVia,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMapping {
    pub todo: ResolvedPhase,
    pub in_progress: ResolvedPhase,
    pub ready_to_test: ResolvedPhase,
    pub done: ResolvedPhase,
    pub blocked: ResolvedPhase,
}

/// First option (discovery order) whose normalized name is in `synonyms`.
fn match_option<'a>(options: &'a [StatusOption], synonyms: &[&str]) -> Option<&'a StatusOption> {
    options
        .iter()
        .find(|o| synonyms.contains(&normalize(&o.name).as_str()))
}

/// Which core phases (Todo / InProgress / Done — the ones with NO fallback)
/// have no synonym match. Pure; the discover route reports these on the wire
/// (`unmappedPhases`, snake_case) when the mapper refuses.
pub fn unmapped_core_phases(options: &[StatusOption]) -> Vec<&'static str> {
    [
        ("todo", TODO_SYNONYMS),
        ("in_progress", IN_PROGRESS_SYNONYMS),
        ("done", DONE_SYNONYMS),
    ]
    .into_iter()
    .filter(|(_, synonyms)| match_option(options, synonyms).is_none())
    .map(|(phase, _)| phase)
    .collect()
}

/// Resolve a discovered Status field's options into the five-phase mapping.
///
/// Exact-normalized synonym match per phase, then exactly two fallbacks
/// (AC 1): `ReadyToTest → InProgress`'s option, `Blocked → InProgress`'s
/// option. **Refusal** when `Todo`, `InProgress`, or `Done` has no match: the
/// `Err` names the unmapped phase(s) AND the discovered option names — never a
/// partial mapping (the UI turns a refusal into manual per-phase selects, D7).
pub fn resolve_status_mapping(options: &[StatusOption]) -> Result<ResolvedMapping, String> {
    let unmapped = unmapped_core_phases(options);
    if !unmapped.is_empty() {
        let names: Vec<&str> = options.iter().map(|o| o.name.as_str()).collect();
        return Err(format!(
            "could not map phase(s) {} onto the project's Status options {:?}; \
             pick the columns manually",
            unmapped.join(", "),
            names
        ));
    }
    let matched = |o: &StatusOption| ResolvedPhase {
        option_id: o.id.clone(),
        option_name: o.name.clone(),
        via: MatchVia::Matched,
    };
    // Refusal above guarantees the three core matches exist.
    let todo = matched(match_option(options, TODO_SYNONYMS).expect("todo matched"));
    let in_progress = matched(match_option(options, IN_PROGRESS_SYNONYMS).expect("ip matched"));
    let done = matched(match_option(options, DONE_SYNONYMS).expect("done matched"));
    let fell_back_to_in_progress = || ResolvedPhase {
        option_id: in_progress.option_id.clone(),
        option_name: in_progress.option_name.clone(),
        via: MatchVia::FellBack,
    };
    let ready_to_test = match_option(options, READY_TO_TEST_SYNONYMS)
        .map(matched)
        .unwrap_or_else(fell_back_to_in_progress);
    let blocked = match_option(options, BLOCKED_SYNONYMS)
        .map(matched)
        .unwrap_or_else(fell_back_to_in_progress);
    Ok(ResolvedMapping {
        todo,
        in_progress,
        ready_to_test,
        done,
        blocked,
    })
}

// ─── Discovery: one `gh api graphql` call ───────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredStatusField {
    pub project_id: String,
    pub project_title: String,
    pub status_field_id: String,
    pub options: Vec<StatusOption>,
}

/// A classified Projects failure. Kinds: `scope_missing` | `auth_required` |
/// `not_found` | `no_status_field` | `network_error` | `unknown`. The
/// `scope_missing` message is CONSTRUCTED to carry the remedy verbatim so
/// every surface (bind route 422, mid-run log line) shows the fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectsError {
    pub kind: &'static str,
    pub message: String,
}

impl ProjectsError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The actionable scope error (spec 010 AC 2d): default `gh` logins lack the
/// `project` scope; the message IS the remedy.
const SCOPE_MISSING_MESSAGE: &str =
    "GitHub Projects needs the `project` token scope. Run: gh auth refresh -s project";

/// `gh` binary override — same knob as `task_sink::gh_bin` (kept local: F1
/// adds nothing to task_sink). Tests pass the program explicitly instead.
pub(crate) fn gh_bin() -> String {
    std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into())
}

/// Pure argv builder for one `gh api graphql` call. String vars ride `-f`
/// (always a string); Int vars ride `-F` (typed) so `$number: Int!` binds as a
/// number — the discipline copied from the desktop `gh_projects.rs::graphql`.
/// Values are argv tokens, never shell-interpolated.
fn gh_graphql_argv(
    query: &str,
    str_vars: &[(&str, &str)],
    int_vars: &[(&str, i64)],
) -> Vec<String> {
    let mut argv = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={query}"),
    ];
    for (key, value) in str_vars {
        argv.push("-f".to_string());
        argv.push(format!("{key}={value}"));
    }
    for (key, value) in int_vars {
        argv.push("-F".to_string());
        argv.push(format!("{key}={value}"));
    }
    argv
}

/// Classify a GraphQL `errors[]` array (gh exits 1 but prints the JSON body).
fn classify_graphql_errors(errors: &[Value]) -> ProjectsError {
    let first = errors.first();
    let typ = first
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = first
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("GitHub Projects request failed.")
        .to_string();
    let lower = message.to_lowercase();
    match typ {
        "INSUFFICIENT_SCOPES" | "FORBIDDEN" => {
            ProjectsError::new("scope_missing", SCOPE_MISSING_MESSAGE)
        }
        "NOT_FOUND" => ProjectsError::new("not_found", message),
        _ if lower.contains("read:project") || lower.contains("required scopes") => {
            ProjectsError::new("scope_missing", SCOPE_MISSING_MESSAGE)
        }
        _ if lower.contains("could not resolve to") || lower.contains("not found") => {
            ProjectsError::new("not_found", message)
        }
        _ => ProjectsError::new("unknown", message),
    }
}

/// `gh` failed before returning a GraphQL body (not installed, not logged in,
/// network). Mirrors the desktop `gh_projects.rs::classify_stderr` heuristics.
fn classify_stderr(stderr: &str) -> ProjectsError {
    let lower = stderr.to_lowercase();
    let first_line = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Couldn't reach GitHub Projects.")
        .to_string();
    if lower.contains("read:project") || lower.contains("required scopes") {
        return ProjectsError::new("scope_missing", SCOPE_MISSING_MESSAGE);
    }
    if lower.contains("gh auth login")
        || lower.contains("not logged in")
        || lower.contains("authentication")
    {
        return ProjectsError::new("auth_required", first_line);
    }
    if lower.contains("could not resolve to") || lower.contains("not found") {
        return ProjectsError::new("not_found", first_line);
    }
    if lower.contains("could not connect") || lower.contains("timeout") {
        return ProjectsError::new("network_error", first_line);
    }
    ProjectsError::new("unknown", first_line)
}

/// Run one GraphQL operation through `gh api graphql` from the neutral cwd
/// (`task_sink::neutral_cwd()` — an explicit-var call makes the cwd's git
/// remote irrelevant). Returns the response's `data`. Bounded by the same 30s
/// timeout as `task_sink::run_gh` so a hung `gh` degrades to a classified
/// error, never a stalled request. `program` is explicit so tests inject a
/// fake `gh` without env mutation.
async fn run_gh_graphql(
    program: &str,
    query: &str,
    str_vars: &[(&str, &str)],
    int_vars: &[(&str, i64)],
) -> Result<Value, ProjectsError> {
    let argv = gh_graphql_argv(query, str_vars, int_vars);
    let fut = tokio::process::Command::new(program)
        .args(&argv)
        .current_dir(crate::task_sink::neutral_cwd())
        .output();
    let output = match tokio::time::timeout(std::time::Duration::from_secs(30), fut).await {
        Err(_) => return Err(ProjectsError::new("network_error", "gh timed out")),
        Ok(Err(e)) => {
            return Err(ProjectsError::new(
                "auth_required",
                format!("GitHub CLI (`gh`) could not be run: {e}"),
            ));
        }
        Ok(Ok(o)) => o,
    };
    // gh prints the JSON body on both success and GraphQL errors; parse it first.
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(body) = serde_json::from_str::<Value>(&stdout) {
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                return Err(classify_graphql_errors(errors));
            }
        }
        match body.get("data") {
            Some(data) if !data.is_null() => return Ok(data.clone()),
            _ => {
                return Err(ProjectsError::new(
                    "unknown",
                    "GitHub returned an empty response.",
                ));
            }
        }
    }
    // No JSON body → gh failed before the request (auth/scope/network).
    Err(classify_stderr(&String::from_utf8_lossy(&output.stderr)))
}

/// `"organization"` / `"user"` are validated values from the picker, safe to
/// interpolate as the GraphQL root field; anything unexpected falls back to
/// `user` (the desktop `owner_node` rule). The login is ALWAYS a `$var`.
fn owner_node(owner_type: &str) -> &'static str {
    if owner_type == "organization" {
        "organization"
    } else {
        "user"
    }
}

/// Pure parse of the discovery `data` (fixture-tested): project id/title + the
/// Status single-select field id + options. A null project → `not_found`; a
/// missing / non-single-select `Status` field → `no_status_field` with an
/// actionable message.
fn parse_discovery(data: &Value) -> Result<DiscoveredStatusField, ProjectsError> {
    let root = data
        .get("organization")
        .or_else(|| data.get("user"))
        .filter(|v| !v.is_null())
        .ok_or_else(|| ProjectsError::new("not_found", "Project owner not found."))?;
    let project = root
        .get("projectV2")
        .filter(|v| !v.is_null())
        .ok_or_else(|| ProjectsError::new("not_found", "Project not found."))?;
    let project_id = project
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProjectsError::new("unknown", "Project has no id."))?
        .to_string();
    let project_title = project
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let field = project
        .get("field")
        .filter(|v| !v.is_null())
        .ok_or_else(|| {
            ProjectsError::new(
                "no_status_field",
                "The project has no \"Status\" field — add a single-select Status field \
             (GitHub creates one by default) and re-discover.",
            )
        })?;
    if field.get("__typename").and_then(Value::as_str) != Some("ProjectV2SingleSelectField") {
        return Err(ProjectsError::new(
            "no_status_field",
            "The project's \"Status\" field is not a single-select field, so it has \
             no options to map phases onto.",
        ));
    }
    let status_field_id = field
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProjectsError::new("unknown", "Status field has no id."))?
        .to_string();
    let options = field
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| {
                    Some(StatusOption {
                        id: o.get("id")?.as_str()?.to_string(),
                        name: o.get("name")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DiscoveredStatusField {
        project_id,
        project_title,
        status_field_id,
        options,
    })
}

/// Discover a project's Status single-select field in ONE `gh api graphql`
/// call — which doubles as the scope probe: a missing `project` scope fails
/// here and classifies to the actionable `scope_missing` (AC 2d). The owner
/// node is the validated root field; the login is always a `$owner` var.
pub async fn discover_status_field(
    program: &str,
    owner: &str,
    owner_type: &str,
    number: i64,
) -> Result<DiscoveredStatusField, ProjectsError> {
    let node = owner_node(owner_type);
    let query = format!(
        "query($owner: String!, $number: Int!) {{ {node}(login: $owner) {{ \
         projectV2(number: $number) {{ id title field(name: \"Status\") {{ __typename \
         ... on ProjectV2SingleSelectField {{ id name options {{ id name }} }} }} }} }} }}"
    );
    let data = run_gh_graphql(program, &query, &[("owner", owner)], &[("number", number)]).await?;
    parse_discovery(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(pairs: &[(&str, &str)]) -> Vec<StatusOption> {
        pairs
            .iter()
            .map(|(id, name)| StatusOption {
                id: id.to_string(),
                name: name.to_string(),
            })
            .collect()
    }

    // ─── The pure fuzzy mapper (AC 2 fixtures — written FIRST) ─────────────

    #[test]
    fn normalize_strips_case_space_punct_emoji() {
        assert_eq!(normalize("🚧 In-Progress "), "inprogress");
        assert_eq!(normalize("Ready to Test"), "readytotest");
        assert_eq!(normalize("QA"), "qa");
        assert_eq!(normalize("Up_Next!"), "upnext");
        assert_eq!(normalize("Done ✅"), "done");
        assert_eq!(normalize("v2 Backlog"), "v2backlog");
        assert_eq!(normalize(""), "");
    }

    /// Structural guard: no normalized token appears in two phase lists —
    /// disjointness is what makes first-hit-wins scanning order-independent
    /// across phases.
    #[test]
    fn synonym_lists_are_disjoint() {
        let lists: [(&str, &[&str]); 5] = [
            ("todo", TODO_SYNONYMS),
            ("in_progress", IN_PROGRESS_SYNONYMS),
            ("ready_to_test", READY_TO_TEST_SYNONYMS),
            ("done", DONE_SYNONYMS),
            ("blocked", BLOCKED_SYNONYMS),
        ];
        let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for (phase, tokens) in lists {
            for token in tokens {
                assert_eq!(
                    normalize(token),
                    *token,
                    "synonym {token:?} must already be normalized"
                );
                if let Some(other) = seen.insert(token, phase) {
                    panic!("token {token:?} appears in both {other} and {phase}");
                }
            }
        }
    }

    /// AC 2 fixture (a): a default board (Todo / In Progress / Done) maps the
    /// three core phases and falls back RTT + Blocked to the In Progress option.
    #[test]
    fn resolve_default_board_maps_three_and_falls_back_rtt_blocked() {
        let options = opts(&[("o1", "Todo"), ("o2", "In Progress"), ("o3", "Done")]);
        let m = resolve_status_mapping(&options).unwrap();
        assert_eq!(m.todo.option_id, "o1");
        assert_eq!(m.todo.via, MatchVia::Matched);
        assert_eq!(m.in_progress.option_id, "o2");
        assert_eq!(m.in_progress.via, MatchVia::Matched);
        assert_eq!(m.done.option_id, "o3");
        assert_eq!(m.done.via, MatchVia::Matched);
        assert_eq!(m.ready_to_test.option_id, "o2");
        assert_eq!(m.ready_to_test.via, MatchVia::FellBack);
        assert_eq!(m.blocked.option_id, "o2");
        assert_eq!(m.blocked.via, MatchVia::FellBack);
    }

    /// AC 2 fixture (b): a custom board (Backlog / Building / QA / Shipped)
    /// resolves ReadyToTest→"QA" and Done→"Shipped"; Blocked falls back to the
    /// Building option.
    #[test]
    fn resolve_custom_backlog_building_qa_shipped() {
        let options = opts(&[
            ("b1", "Backlog"),
            ("b2", "Building"),
            ("b3", "QA"),
            ("b4", "Shipped"),
        ]);
        let m = resolve_status_mapping(&options).unwrap();
        assert_eq!(m.todo.option_id, "b1");
        assert_eq!(m.in_progress.option_id, "b2");
        assert_eq!(m.ready_to_test.option_id, "b3");
        assert_eq!(m.ready_to_test.option_name, "QA");
        assert_eq!(m.ready_to_test.via, MatchVia::Matched);
        assert_eq!(m.done.option_id, "b4");
        assert_eq!(m.done.option_name, "Shipped");
        assert_eq!(m.blocked.option_id, "b2");
        assert_eq!(m.blocked.via, MatchVia::FellBack);
    }

    /// AC 2 fixture (c): no ReadyToTest-like column → RTT resolves to the
    /// InProgress option, flagged FellBack (the D5 visible-hint contract).
    #[test]
    fn resolve_no_rtt_column_falls_back_to_in_progress_option() {
        let options = opts(&[
            ("c1", "Triage"),
            ("c2", "🚧 In-Progress"),
            ("c3", "Blocked"),
            ("c4", "Released"),
        ]);
        let m = resolve_status_mapping(&options).unwrap();
        assert_eq!(m.ready_to_test.option_id, "c2");
        assert_eq!(m.ready_to_test.option_name, "🚧 In-Progress");
        assert_eq!(m.ready_to_test.via, MatchVia::FellBack);
        // Blocked has its own column here — no fallback for it.
        assert_eq!(m.blocked.option_id, "c3");
        assert_eq!(m.blocked.via, MatchVia::Matched);
    }

    /// Refusal: a core phase (Todo/InProgress/Done) with no match returns Err
    /// naming the phase(s) AND the discovered option names — never a partial
    /// mapping. Substring non-matching is pinned here too ("Not Started" must
    /// NOT match "started", "Not Done" must NOT match "done").
    #[test]
    fn resolve_refuses_when_core_phase_unmappable_never_partial() {
        let options = opts(&[("x1", "Weird"), ("x2", "Columns"), ("x3", "Here")]);
        let err = resolve_status_mapping(&options).unwrap_err();
        for phase in ["todo", "in_progress", "done"] {
            assert!(err.contains(phase), "names {phase}: {err}");
        }
        assert!(err.contains("Weird"), "lists the option names: {err}");

        // Exact-normalized matching only: negated names never false-positive.
        let negated = opts(&[("n1", "Not Started"), ("n2", "Not Done"), ("n3", "Todo")]);
        let err = resolve_status_mapping(&negated).unwrap_err();
        assert!(
            err.contains("in_progress") && err.contains("done"),
            "negated names must not match: {err}"
        );
        assert_eq!(unmapped_core_phases(&negated), vec!["in_progress", "done"]);
    }

    // ─── Discovery plumbing ────────────────────────────────────────────────

    #[test]
    fn gh_graphql_argv_uses_f_for_strings_big_f_for_ints() {
        let argv = gh_graphql_argv("query(...)", &[("owner", "acme")], &[("number", 7)]);
        assert_eq!(
            argv,
            vec![
                "api",
                "graphql",
                "-f",
                "query=query(...)",
                "-f",
                "owner=acme",
                "-F",
                "number=7",
            ]
        );
    }

    #[test]
    fn parse_discovery_extracts_field_and_options() {
        let data = json!({
            "organization": {
                "projectV2": {
                    "id": "PVT_kwDO1",
                    "title": "Widgets",
                    "field": {
                        "__typename": "ProjectV2SingleSelectField",
                        "id": "PVTSSF_1",
                        "name": "Status",
                        "options": [
                            {"id": "f75ad846", "name": "Todo"},
                            {"id": "47fc9ee4", "name": "In Progress"},
                            {"id": "98236657", "name": "Done"}
                        ]
                    }
                }
            }
        });
        let d = parse_discovery(&data).unwrap();
        assert_eq!(d.project_id, "PVT_kwDO1");
        assert_eq!(d.project_title, "Widgets");
        assert_eq!(d.status_field_id, "PVTSSF_1");
        assert_eq!(d.options.len(), 3);
        assert_eq!(d.options[1].id, "47fc9ee4");
        assert_eq!(d.options[1].name, "In Progress");

        // The `user` root parses identically (the picker's ownerType decides).
        let user_data = json!({
            "user": { "projectV2": { "id": "PVT_u", "title": "T", "field": {
                "__typename": "ProjectV2SingleSelectField", "id": "F", "name": "Status",
                "options": [{"id": "a", "name": "Todo"}]
            } } }
        });
        assert_eq!(parse_discovery(&user_data).unwrap().project_id, "PVT_u");
    }

    #[test]
    fn parse_discovery_missing_status_field_is_actionable() {
        let data = json!({
            "organization": { "projectV2": { "id": "PVT_1", "title": "T", "field": null } }
        });
        let err = parse_discovery(&data).unwrap_err();
        assert_eq!(err.kind, "no_status_field");
        assert!(
            err.message.contains("Status"),
            "actionable: {}",
            err.message
        );

        // A non-single-select "Status" field is the same actionable kind.
        let text_field = json!({
            "organization": { "projectV2": { "id": "PVT_1", "title": "T", "field": {
                "__typename": "ProjectV2Field", "id": "F", "name": "Status"
            } } }
        });
        let err = parse_discovery(&text_field).unwrap_err();
        assert_eq!(err.kind, "no_status_field");

        // A null project is not_found, never a partial result.
        let no_project = json!({ "organization": { "projectV2": null } });
        assert_eq!(parse_discovery(&no_project).unwrap_err().kind, "not_found");
    }

    /// AC 2 fixture (d), the scope half: both classifier inputs (a GraphQL
    /// `errors[]` body and a bare stderr) classify to `scope_missing` with the
    /// remedy command in the message.
    #[test]
    fn classify_scope_missing_names_gh_auth_refresh() {
        let errors = vec![json!({
            "type": "INSUFFICIENT_SCOPES",
            "message": "Your token has not been granted the required scopes to execute this query. The 'projectV2' field requires one of the following scopes: ['read:project']"
        })];
        let err = classify_graphql_errors(&errors);
        assert_eq!(err.kind, "scope_missing");
        assert!(
            err.message.contains("gh auth refresh -s project"),
            "carries the remedy: {}",
            err.message
        );

        let err = classify_stderr(
            "error: your token is missing required scopes [read:project]\nhint: run gh auth refresh",
        );
        assert_eq!(err.kind, "scope_missing");
        assert!(err.message.contains("gh auth refresh -s project"));

        // The neighboring kinds stay distinct.
        assert_eq!(
            classify_stderr("To get started with GitHub CLI, please run: gh auth login").kind,
            "auth_required"
        );
        assert_eq!(
            classify_stderr("Could not resolve to an Organization with the login of 'acme'.").kind,
            "not_found"
        );
        assert_eq!(
            classify_graphql_errors(&[json!({
                "type": "NOT_FOUND",
                "message": "Could not resolve to a ProjectV2 with the number of 99."
            })])
            .kind,
            "not_found"
        );
    }

    /// End-to-end discovery against a fake `gh` (the established task_sink
    /// pattern): ONE invocation, canned JSON in, parsed result out.
    #[cfg(unix)]
    #[tokio::test]
    async fn discover_status_field_with_fake_gh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let body = json!({
            "data": { "organization": { "projectV2": {
                "id": "PVT_kwDO9", "title": "Roadmap",
                "field": {
                    "__typename": "ProjectV2SingleSelectField",
                    "id": "PVTSSF_9", "name": "Status",
                    "options": [
                        {"id": "aa", "name": "Backlog"},
                        {"id": "bb", "name": "Building"},
                        {"id": "cc", "name": "QA"},
                        {"id": "dd", "name": "Shipped"}
                    ]
                }
            } } }
        });
        let script = dir.path().join("gh-fake");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\ncat <<'EOF'\n{}\nEOF\n",
                log.display(),
                body
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let d = discover_status_field(script.to_str().unwrap(), "acme", "organization", 7)
            .await
            .unwrap();
        assert_eq!(d.project_id, "PVT_kwDO9");
        assert_eq!(d.status_field_id, "PVTSSF_9");
        assert_eq!(d.options.len(), 4);
        assert_eq!(d.options[2].name, "QA");

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 1, "discovery is ONE gh call, got: {calls}");
        assert!(
            lines[0].starts_with("api graphql -f query="),
            "got: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("organization(login: $owner)"),
            "org root field: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("-f owner=acme"),
            "login is a var: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("-F number=7"),
            "number is typed: {}",
            lines[0]
        );
    }

    // ─── Persistence (path-injected — never the real config file) ──────────

    fn sample_binding() -> BoardBinding {
        BoardBinding {
            project_id: "PVT_1".into(),
            status_field_id: "PVTSSF_1".into(),
            status_mapping: StatusMapping {
                todo: "t".into(),
                in_progress: "i".into(),
                ready_to_test: "r".into(),
                done: "d".into(),
                blocked: "b".into(),
            },
            done_closes_issue: true,
            project_title: Some("Widgets".into()),
            project_owner: Some("acme".into()),
            project_owner_type: Some("organization".into()),
            project_number: Some(7),
            option_names: None,
        }
    }

    /// AC 2d on the storage side: a stored mapping missing a phase fails
    /// deserialization, so the read degrades to "no binding" — a partial
    /// binding is unrepresentable on disk too.
    #[test]
    fn stored_binding_missing_phase_fails_deserialize_reads_as_no_binding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github_projects.json");
        std::fs::write(
            &path,
            r#"{ "bindings": { "acme/widgets": {
                "project_id": "PVT_1", "status_field_id": "PVTSSF_1",
                "status_mapping": { "todo": "t", "in_progress": "i",
                                    "ready_to_test": "r", "done": "d" }
            } } }"#,
        )
        .unwrap();
        assert_eq!(binding_for_slug_at(&path, "acme/widgets"), None);
    }

    #[test]
    fn binding_for_slug_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github_projects.json");
        upsert_binding_at(&path, "Acme/Widgets", sample_binding()).unwrap();
        assert!(binding_for_slug_at(&path, "acme/widgets").is_some());
        assert!(binding_for_slug_at(&path, "ACME/WIDGETS").is_some());
        assert!(binding_for_slug_at(&path, " acme/widgets ").is_some());
        assert!(binding_for_slug_at(&path, "acme/other").is_none());
    }

    #[test]
    fn upsert_preserves_other_slugs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github_projects.json");
        upsert_binding_at(&path, "acme/widgets", sample_binding()).unwrap();
        let mut second = sample_binding();
        second.project_id = "PVT_2".into();
        upsert_binding_at(&path, "acme/gadgets", second).unwrap();
        // Replace the first — the second must survive the RMW.
        let mut replaced = sample_binding();
        replaced.project_id = "PVT_3".into();
        upsert_binding_at(&path, "acme/widgets", replaced).unwrap();

        assert_eq!(
            binding_for_slug_at(&path, "acme/widgets")
                .unwrap()
                .project_id,
            "PVT_3"
        );
        assert_eq!(
            binding_for_slug_at(&path, "acme/gadgets")
                .unwrap()
                .project_id,
            "PVT_2"
        );
        // Remove one; the other stays. Removing a missing slug is Ok(false).
        assert_eq!(remove_binding_at(&path, "acme/widgets"), Ok(true));
        assert_eq!(binding_for_slug_at(&path, "acme/widgets"), None);
        assert!(binding_for_slug_at(&path, "acme/gadgets").is_some());
        assert_eq!(remove_binding_at(&path, "acme/widgets"), Ok(false));
    }

    /// D1/§7.7: the knob's ON default lives in the serde default — a binding
    /// stored without it (or fetched from an older file) reads true.
    #[test]
    fn done_closes_issue_defaults_true_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github_projects.json");
        std::fs::write(
            &path,
            r#"{ "bindings": { "acme/widgets": {
                "project_id": "PVT_1", "status_field_id": "PVTSSF_1",
                "status_mapping": { "todo": "t", "in_progress": "i",
                                    "ready_to_test": "r", "done": "d", "blocked": "b" }
            } } }"#,
        )
        .unwrap();
        let b = binding_for_slug_at(&path, "acme/widgets").unwrap();
        assert!(b.done_closes_issue, "absent knob must read ON");
        // And an explicit false round-trips as false.
        let mut off = sample_binding();
        off.done_closes_issue = false;
        upsert_binding_at(&path, "x/y", off).unwrap();
        assert!(!binding_for_slug_at(&path, "x/y").unwrap().done_closes_issue);
    }

    #[test]
    fn board_phase_from_tracker_phase_covers_four() {
        use crate::task_sink::TrackerPhase;
        assert_eq!(BoardPhase::from(TrackerPhase::Todo), BoardPhase::Todo);
        assert_eq!(
            BoardPhase::from(TrackerPhase::InProgress),
            BoardPhase::InProgress
        );
        assert_eq!(
            BoardPhase::from(TrackerPhase::ReadyToTest),
            BoardPhase::ReadyToTest
        );
        assert_eq!(BoardPhase::from(TrackerPhase::Done), BoardPhase::Done);
    }

    #[test]
    fn status_mapping_option_id_covers_all_phases() {
        let m = sample_binding().status_mapping;
        assert_eq!(m.option_id(BoardPhase::Todo), "t");
        assert_eq!(m.option_id(BoardPhase::InProgress), "i");
        assert_eq!(m.option_id(BoardPhase::ReadyToTest), "r");
        assert_eq!(m.option_id(BoardPhase::Done), "d");
        assert_eq!(m.option_id(BoardPhase::Blocked), "b");
    }
}
