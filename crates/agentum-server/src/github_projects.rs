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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

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
    InReview,
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
            InReview => BoardPhase::InReview,
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
    /// #379: the "In Review / PR" column option — the card lands here when a
    /// non-draft PR opens (tracker_sync's PR-open → InReview). `#[serde(default)]`
    /// (empty) so every pre-#379 binding on disk deserializes and behaves
    /// EXACTLY as before: `option_id(InReview)` falls back to `in_progress`
    /// until the binding is re-discovered with a Review/PR column.
    #[serde(default)]
    pub in_review: String,
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
            // #379: InReview lands on its own "In Review / PR" column when the
            // binding maps one; an UNMAPPED in_review (empty — every pre-#379
            // binding) folds back onto InProgress, spec 012 F3's original
            // behavior, so existing boards are byte-identical until re-bound.
            BoardPhase::InReview => {
                if self.in_review.trim().is_empty() {
                    &self.in_progress
                } else {
                    &self.in_review
                }
            }
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
    pub in_review: String,
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

/// Upgrade a pre-InReview binding on demand. Older bindings deserialize with
/// an empty `in_review`, which historically folded PRs onto In Progress. Once
/// the project exposes a Review-like option, delivery must discover and persist
/// that option before an InReview write can be acknowledged.
fn binding_with_discovered_in_review(
    binding: &BoardBinding,
    discovered: &DiscoveredStatusField,
) -> Result<BoardBinding, String> {
    if discovered.project_id != binding.project_id
        || discovered.status_field_id != binding.status_field_id
    {
        return Err(
            "the saved project binding no longer matches the discovered Status field; rebind the tracker"
                .into(),
        );
    }
    let resolved = resolve_status_mapping(&discovered.options)?;
    if resolved.in_review.via != MatchVia::Matched {
        // A board with no review column intentionally keeps the documented
        // InReview → InProgress fallback. Discovery is still required to tell
        // that valid choice apart from a stale pre-InReview binding.
        return Ok(binding.clone());
    }

    let mut upgraded = binding.clone();
    upgraded.status_mapping.in_review = resolved.in_review.option_id;
    let mut names = upgraded.option_names.take().unwrap_or_default();
    names.in_review = resolved.in_review.option_name;
    upgraded.option_names = Some(names);
    Ok(upgraded)
}

/// Return the effective binding for an InReview write. Current bindings are a
/// zero-call fast path; legacy bindings perform discovery and persist a newly
/// matched review option. Boards without one retain the documented InProgress
/// fallback.
pub async fn ensure_in_review_mapping(
    program: &str,
    slug: &str,
    binding: &BoardBinding,
) -> Result<BoardBinding, String> {
    if !binding.status_mapping.in_review.trim().is_empty() {
        return Ok(binding.clone());
    }
    let owner = binding
        .project_owner
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "legacy project binding has no owner metadata; rebind the tracker".to_string()
        })?;
    let owner_type = binding
        .project_owner_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "legacy project binding has no owner type metadata; rebind the tracker".to_string()
        })?;
    let number = binding.project_number.ok_or_else(|| {
        "legacy project binding has no project number; rebind the tracker".to_string()
    })?;
    let discovered = discover_status_field(program, owner, owner_type, number)
        .await
        .map_err(|error| error.message)?;
    let upgraded = binding_with_discovered_in_review(binding, &discovered)?;
    upsert_binding(slug, upgraded.clone())?;
    Ok(upgraded)
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
// #379: a distinct "In Review / PR" phase — the card lands here on PR-open.
// The review/PR-flavored tokens moved OUT of READY_TO_TEST into here (kept
// disjoint, pinned by `synonym_lists_are_disjoint`): a "Review"/"PR" column is
// the PR/review stage, while QA/test columns stay ready_to_test.
const IN_REVIEW_SYNONYMS: &[&str] = &[
    "inreview",
    "review",
    "readyforreview",
    "codereview",
    "reviewing",
    "pr",
    "pullrequest",
    "inpr",
];
const READY_TO_TEST_SYNONYMS: &[&str] = &[
    "readytotest",
    "readyfortest",
    "qa",
    "testing",
    "test",
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
    pub in_review: ResolvedPhase,
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
    // #379: In Review matches a Review/PR column, else folds onto In Progress
    // (same nearest-earlier fallback as ready_to_test/blocked — and the exact
    // pre-#379 InReview→InProgress behavior when no such column exists).
    let in_review = match_option(options, IN_REVIEW_SYNONYMS)
        .map(matched)
        .unwrap_or_else(fell_back_to_in_progress);
    let ready_to_test = match_option(options, READY_TO_TEST_SYNONYMS)
        .map(matched)
        .unwrap_or_else(fell_back_to_in_progress);
    let blocked = match_option(options, BLOCKED_SYNONYMS)
        .map(matched)
        .unwrap_or_else(fell_back_to_in_progress);
    Ok(ResolvedMapping {
        todo,
        in_progress,
        in_review,
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
    run_gh_graphql_argv(program, &gh_graphql_argv(query, str_vars, int_vars)).await
}

/// The argv-level GraphQL runner + classifier — F1 discovery and every F2
/// board write ride THIS one path (§4.2: ONE runner + ONE classifier serve
/// every call), so a scope/auth/network miss classifies identically at bind
/// time and mid-run.
async fn run_gh_graphql_argv(program: &str, argv: &[String]) -> Result<Value, ProjectsError> {
    let fut =
        crate::task_sink::output_with_etxtbsy_retry(program, argv, crate::task_sink::neutral_cwd());
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

// ─── F2: the board writes (drive) ───────────────────────────────────────────
//
// One transition's whole board side: ensure the issue is a project item
// (cached), write the mapped Status OPTION ID, then the knob-gated
// probe-then-act close/reopen. Called from `task_sink`'s github arms via
// `github_transition_with_board` / `github_mark_blocked_with_board` — the
// caller folds a returned reason into the transition report, so nothing here
// Err-propagates past that string or panics (the best-effort contract, AC 7).

// The three GraphQL operations, single-line (no newlines) so a fake-gh call
// log stays one line per invocation — the same property the discovery query
// relies on. All GA-stable since 2022 (§6.2).
const ISSUE_NODE_ID_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { \
     repository(owner: $owner, name: $name) { issue(number: $number) { id } } }";
const ADD_ITEM_MUTATION: &str = "mutation($project: ID!, $content: ID!) { \
     addProjectV2ItemById(input: {projectId: $project, contentId: $content}) { item { id } } }";
const UPDATE_STATUS_MUTATION: &str = "mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) { \
     updateProjectV2ItemFieldValue(input: {projectId: $project, itemId: $item, \
     fieldId: $field, value: {singleSelectOptionId: $option}}) { projectV2Item { id } } }";

/// Pure argv: resolve an issue's GraphQL node id (§4.2 step 2). GraphQL, not
/// REST, so the one runner + one classifier above serve this call too.
fn issue_node_id_query_args(owner: &str, name: &str, number: i64) -> Vec<String> {
    gh_graphql_argv(
        ISSUE_NODE_ID_QUERY,
        &[("owner", owner), ("name", name)],
        &[("number", number)],
    )
}

/// Pure argv: ensure-on-board + item id in ONE call (§4.2 step 3) —
/// `addProjectV2ItemById` is idempotent by API contract (re-adding returns the
/// existing item's id), so this is both the ensure AND the fetch. It is also
/// what makes a chat-filed issue land in the Todo column (AC 11): the Todo
/// transition's lazy ensure.
fn add_item_mutation_args(project_id: &str, content_id: &str) -> Vec<String> {
    gh_graphql_argv(
        ADD_ITEM_MUTATION,
        &[("project", project_id), ("content", content_id)],
        &[],
    )
}

/// Pure argv: the option write (§4.2 step 4). `option_id` is the STORED
/// single-select option id — option IDs, never names, at write time (PRD
/// AC 6): column renames after bind still land.
fn update_status_mutation_args(
    project_id: &str,
    item_id: &str,
    field_id: &str,
    option_id: &str,
) -> Vec<String> {
    gh_graphql_argv(
        UPDATE_STATUS_MUTATION,
        &[
            ("project", project_id),
            ("item", item_id),
            ("field", field_id),
            ("option", option_id),
        ],
        &[],
    )
}

/// Probe argv (§4.2 step 6): `--jq .state` makes stdout the bare
/// `OPEN`/`CLOSED` token, so probe-then-act needs no JSON parse.
fn gh_issue_state_argv<'a>(number: &'a str, slug: &'a str) -> [&'a str; 9] {
    [
        "issue", "view", number, "--repo", slug, "--json", "state", "--jq", ".state",
    ]
}

fn gh_issue_close_argv<'a>(number: &'a str, slug: &'a str) -> [&'a str; 5] {
    ["issue", "close", number, "--repo", slug]
}

fn gh_issue_reopen_argv<'a>(number: &'a str, slug: &'a str) -> [&'a str; 5] {
    ["issue", "reopen", number, "--repo", slug]
}

/// One plain (non-GraphQL) `gh` call from the neutral cwd, RETURNING stdout —
/// the stdout-carrying sibling of `task_sink::run_gh` (which discards stdout
/// and stays untouched). Same 30s bound and ~240-char stderr truncation so a
/// hung or failing `gh` degrades to a reason string, never a stalled
/// transition.
async fn run_gh_capture(program: &str, args: &[&str]) -> Result<String, String> {
    let fut =
        crate::task_sink::output_with_etxtbsy_retry(program, args, crate::task_sink::neutral_cwd());
    let output = match tokio::time::timeout(std::time::Duration::from_secs(30), fut).await {
        Err(_) => return Err("gh timed out".into()),
        Ok(Err(e)) => return Err(format!("failed to run `{program}`: {e}")),
        Ok(Ok(o)) => o,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let mut msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if msg.is_empty() {
        msg = format!("gh exited with {}", output.status);
    }
    if msg.len() > 240 {
        let mut end = 240;
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        msg.truncate(end);
        msg.push('…');
    }
    Err(msg)
}

/// `(lowercase slug, issue number)` → `(issue node id, project item id)`.
type IdCacheMap = HashMap<(String, String), (String, String)>;

/// Process-lifetime id cache (§7.3). No TTL: issue node ids are immutable and
/// item ids die only when a card is removed from the board, which the
/// invalidate-and-retry-once path in [`board_write_with`] heals. The cache is
/// what keeps a bound feature run inside the spec's ≤ ~10-gh-calls ceiling
/// (9 warm vs ~14 cold); correctness NEVER depends on it.
static ID_CACHE: LazyLock<Mutex<IdCacheMap>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_key(slug: &str, number: &str) -> (String, String) {
    (slug.trim().to_lowercase(), number.trim().to_string())
}

/// The cold path (§4.2 steps 2–3): issue node id → `addProjectV2ItemById`.
/// Populates the cache on success ONLY, so a failed resolve can never poison
/// it. Returns `(issue_node_id, item_id)`.
async fn ensure_item_cold(
    program: &str,
    binding: &BoardBinding,
    slug: &str,
    number: &str,
) -> Result<(String, String), String> {
    let (owner, name) = slug
        .split_once('/')
        .ok_or_else(|| format!("malformed repo slug {slug:?}"))?;
    let number_int: i64 = number
        .trim()
        .parse()
        .map_err(|_| format!("issue number {number:?} is not numeric"))?;
    let data = run_gh_graphql_argv(program, &issue_node_id_query_args(owner, name, number_int))
        .await
        .map_err(|e| e.message)?;
    let node_id = data
        .pointer("/repository/issue/id")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("GitHub returned no node id for issue #{number} in {slug}"))?
        .to_string();
    let data = run_gh_graphql_argv(
        program,
        &add_item_mutation_args(&binding.project_id, &node_id),
    )
    .await
    .map_err(|e| e.message)?;
    let item_id = data
        .pointer("/addProjectV2ItemById/item/id")
        .and_then(Value::as_str)
        .ok_or_else(|| "addProjectV2ItemById returned no item id".to_string())?
        .to_string();
    ID_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(cache_key(slug, number), (node_id.clone(), item_id.clone()));
    Ok((node_id, item_id))
}

/// One transition's whole board side (spec 010 F2, §4.2). Best-effort:
/// `Ok(())` or a reason string the caller folds into `TransitionResult` —
/// never Err-propagates beyond that string, never panics.
///
/// Steps: cached ids (cold ⇒ resolve + ensure-on-board) → the option write
/// (every call; IDs, never names) → on an option-write failure against a
/// CACHED item id, invalidate and retry ONCE cold → the knob-gated
/// probe-then-act close/reopen (Done/InProgress only — Blocked and the rest
/// never touch issue state).
pub async fn board_write_with(
    program: &str,
    binding: &BoardBinding,
    slug: &str,
    number: &str,
    phase: BoardPhase,
) -> Result<(), String> {
    let key = cache_key(slug, number);
    let cached = ID_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key)
        .cloned();
    let (item_id, from_cache) = match cached {
        Some((_node_id, item_id)) => (item_id, true),
        None => (
            ensure_item_cold(program, binding, slug, number).await?.1,
            false,
        ),
    };
    let option_id = binding.status_mapping.option_id(phase);
    let update = run_gh_graphql_argv(
        program,
        &update_status_mutation_args(
            &binding.project_id,
            &item_id,
            &binding.status_field_id,
            option_id,
        ),
    )
    .await;
    if let Err(e) = update {
        if !from_cache {
            return Err(e.message);
        }
        // Stale-cache self-heal (§7.3): a card removed from the board kills
        // its item id. Invalidate and retry ONCE cold — correctness never
        // depends on the cache.
        tracing::warn!(
            slug,
            number,
            reason = %e.message,
            "Projects option write failed on a cached item id; retrying cold once"
        );
        ID_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        let (_node_id, item_id) = ensure_item_cold(program, binding, slug, number).await?;
        run_gh_graphql_argv(
            program,
            &update_status_mutation_args(
                &binding.project_id,
                &item_id,
                &binding.status_field_id,
                option_id,
            ),
        )
        .await
        .map_err(|e| e.message)?;
    }
    // D1/AC 6: close at Done, reopen at InProgress — only when the binding's
    // knob is ON. Knob OFF never probes, so a human-closed issue is respected.
    if binding.done_closes_issue {
        close_or_reopen_for(program, slug, number, phase).await?;
    }
    Ok(())
}

/// §4.2 step 6 — probe-then-act for BOTH directions (§7.4): one
/// `gh issue view --json state` probe kills the exit-nonzero noise a blind
/// close/reopen would produce on every ordinary transition. Only Done and
/// InProgress act; every other phase (including Blocked) is a no-op. A probe
/// failure skips the act with a warn (best-effort); an act failure surfaces
/// as the returned reason so the run log stays loud.
async fn close_or_reopen_for(
    program: &str,
    slug: &str,
    number: &str,
    phase: BoardPhase,
) -> Result<(), String> {
    if !matches!(phase, BoardPhase::Done | BoardPhase::InProgress) {
        return Ok(());
    }
    let state = match run_gh_capture(program, &gh_issue_state_argv(number, slug)).await {
        // `--jq .state` prints the bare token; trim + strip quotes defensively.
        Ok(out) => out.trim().trim_matches('"').to_ascii_uppercase(),
        Err(reason) => {
            tracing::warn!(
                slug,
                number,
                %reason,
                "issue state probe failed; skipping close/reopen (best-effort)"
            );
            return Ok(());
        }
    };
    match phase {
        BoardPhase::Done if state == "OPEN" => {
            run_gh_capture(program, &gh_issue_close_argv(number, slug))
                .await
                .map(drop)
                .map_err(|r| format!("issue close failed: {r}"))
        }
        BoardPhase::InProgress if state == "CLOSED" => {
            run_gh_capture(program, &gh_issue_reopen_argv(number, slug))
                .await
                .map(drop)
                .map_err(|r| format!("issue reopen failed: {r}"))
        }
        _ => Ok(()),
    }
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
        let lists: [(&str, &[&str]); 6] = [
            ("todo", TODO_SYNONYMS),
            ("in_progress", IN_PROGRESS_SYNONYMS),
            ("in_review", IN_REVIEW_SYNONYMS),
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
    fn option_id_in_review_falls_back_to_in_progress_when_unmapped() {
        // #379: an empty in_review (every pre-#379 binding) folds onto the In
        // Progress option — byte-identical to spec 012 F3.
        let m = StatusMapping {
            todo: "t".into(),
            in_progress: "ip".into(),
            in_review: String::new(),
            ready_to_test: "rtt".into(),
            done: "d".into(),
            blocked: "b".into(),
        };
        assert_eq!(m.option_id(BoardPhase::InReview), "ip");
        // A mapped in_review is used verbatim.
        let m2 = StatusMapping {
            in_review: "pr".into(),
            ..m
        };
        assert_eq!(m2.option_id(BoardPhase::InReview), "pr");
    }

    #[test]
    fn resolve_maps_review_and_pr_columns_to_in_review_not_ready_to_test() {
        // #379: a "Review"/"PR" column is the InReview target; QA/Test stays
        // ready_to_test. (Pre-#379 these folded into ready_to_test.)
        for review_name in ["Review", "In Review", "PR", "Pull Request", "Code Review"] {
            let options = opts(&[
                ("o1", "Todo"),
                ("o2", "In Progress"),
                ("o3", review_name),
                ("o4", "QA"),
                ("o5", "Done"),
            ]);
            let m = resolve_status_mapping(&options).unwrap();
            assert_eq!(
                m.in_review.option_id, "o3",
                "{review_name} should map to in_review"
            );
            assert_eq!(m.in_review.via, MatchVia::Matched);
            assert_eq!(m.ready_to_test.option_id, "o4", "QA stays ready_to_test");
        }
    }

    #[test]
    fn resolve_in_review_falls_back_to_in_progress_without_a_review_column() {
        // No Review/PR column → in_review folds onto In Progress (the exact
        // pre-#379 InReview behavior), reported as FellBack for the UI hint.
        let options = opts(&[("o1", "Todo"), ("o2", "In Progress"), ("o3", "Done")]);
        let m = resolve_status_mapping(&options).unwrap();
        assert_eq!(m.in_review.option_id, "o2");
        assert_eq!(m.in_review.via, MatchVia::FellBack);
    }

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
                in_review: String::new(),
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

    #[test]
    fn legacy_binding_learns_a_discovered_in_review_option() {
        let binding = sample_binding();
        let discovered = DiscoveredStatusField {
            project_id: binding.project_id.clone(),
            project_title: "Widgets".into(),
            status_field_id: binding.status_field_id.clone(),
            options: opts(&[
                ("t", "Backlog"),
                ("i", "In progress"),
                ("review", "In Review"),
                ("r", "QA"),
                ("d", "Done"),
            ]),
        };

        let upgraded = binding_with_discovered_in_review(&binding, &discovered).unwrap();
        assert_eq!(upgraded.status_mapping.in_review, "review");
        assert_eq!(
            upgraded
                .option_names
                .as_ref()
                .map(|names| names.in_review.as_str()),
            Some("In Review")
        );
    }

    #[test]
    fn legacy_binding_preserves_fallback_when_project_has_no_review_option() {
        let binding = sample_binding();
        let discovered = DiscoveredStatusField {
            project_id: binding.project_id.clone(),
            project_title: "Widgets".into(),
            status_field_id: binding.status_field_id.clone(),
            options: opts(&[("t", "Todo"), ("i", "In progress"), ("d", "Done")]),
        };

        let effective = binding_with_discovered_in_review(&binding, &discovered).unwrap();
        assert!(effective.status_mapping.in_review.is_empty());
        assert_eq!(
            effective.status_mapping.option_id(BoardPhase::InReview),
            "i"
        );
    }

    #[test]
    fn legacy_binding_refuses_discovery_from_a_different_project() {
        let binding = sample_binding();
        let discovered = DiscoveredStatusField {
            project_id: "PVT_other".into(),
            project_title: "Other".into(),
            status_field_id: binding.status_field_id.clone(),
            options: Vec::new(),
        };

        let error = binding_with_discovered_in_review(&binding, &discovered).unwrap_err();
        assert!(error.contains("no longer matches"));
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
    fn board_phase_from_tracker_phase_covers_five() {
        use crate::task_sink::TrackerPhase;
        assert_eq!(BoardPhase::from(TrackerPhase::Todo), BoardPhase::Todo);
        assert_eq!(
            BoardPhase::from(TrackerPhase::InProgress),
            BoardPhase::InProgress
        );
        assert_eq!(
            BoardPhase::from(TrackerPhase::InReview),
            BoardPhase::InReview
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
        // Spec 012 F3: InReview folds onto the InProgress option (nearest-earlier
        // fallback — no separate stored option, backward-compatible with 010).
        assert_eq!(m.option_id(BoardPhase::InReview), "i");
        assert_eq!(m.option_id(BoardPhase::ReadyToTest), "r");
        assert_eq!(m.option_id(BoardPhase::Done), "d");
        assert_eq!(m.option_id(BoardPhase::Blocked), "b");
    }

    // ─── F2: pure builders (argv pins) ─────────────────────────────────────

    #[test]
    fn issue_node_id_query_args_shape() {
        let argv = issue_node_id_query_args("acme", "widgets", 42);
        assert_eq!(argv[..3], ["api", "graphql", "-f"]);
        assert!(argv[3].starts_with("query=query($owner: String!, $name: String!, $number: Int!)"));
        assert!(argv[3].contains("repository(owner: $owner, name: $name)"));
        assert!(argv[3].contains("issue(number: $number) { id }"));
        assert_eq!(
            argv[4..],
            ["-f", "owner=acme", "-f", "name=widgets", "-F", "number=42"]
        );
    }

    #[test]
    fn add_item_mutation_args_shape() {
        let argv = add_item_mutation_args("PVT_1", "I_node");
        assert_eq!(argv[..3], ["api", "graphql", "-f"]);
        assert!(
            argv[3].contains(
                "addProjectV2ItemById(input: {projectId: $project, contentId: $content})"
            ),
        );
        assert!(
            argv[3].contains("item { id }"),
            "returns the item id — the ensure AND the fetch: {}",
            argv[3]
        );
        assert_eq!(argv[4..], ["-f", "project=PVT_1", "-f", "content=I_node"]);
    }

    /// PRD AC 6: the write carries the OPTION ID — a `String!` var bound to
    /// `singleSelectOptionId` — never a name.
    #[test]
    fn update_status_mutation_uses_option_id() {
        let argv = update_status_mutation_args("PVT_1", "PVTI_9", "PVTSSF_1", "98236657");
        assert_eq!(argv[..3], ["api", "graphql", "-f"]);
        assert!(argv[3].contains("updateProjectV2ItemFieldValue"));
        assert!(argv[3].contains("value: {singleSelectOptionId: $option}"));
        assert_eq!(
            argv[4..],
            [
                "-f",
                "project=PVT_1",
                "-f",
                "item=PVTI_9",
                "-f",
                "field=PVTSSF_1",
                "-f",
                "option=98236657"
            ]
        );
    }

    #[test]
    fn gh_issue_close_reopen_state_argv_shapes() {
        assert_eq!(
            gh_issue_state_argv("42", "acme/widgets"),
            [
                "issue",
                "view",
                "42",
                "--repo",
                "acme/widgets",
                "--json",
                "state",
                "--jq",
                ".state"
            ]
        );
        assert_eq!(
            gh_issue_close_argv("42", "acme/widgets"),
            ["issue", "close", "42", "--repo", "acme/widgets"]
        );
        assert_eq!(
            gh_issue_reopen_argv("42", "acme/widgets"),
            ["issue", "reopen", "42", "--repo", "acme/widgets"]
        );
    }

    // ─── F2: board_write_with (fake gh) ────────────────────────────────────
    //
    // ID_CACHE is process-global and the test binary is one process, so every
    // test below uses its OWN slug/number — the established pattern for
    // global-state tests (no #[cfg(test)] clear helper, no cross-talk).

    /// A binding whose option ids are self-describing (`opt-<phase>`), so a
    /// fake-gh call log pins WHICH phase's option rode the write.
    #[cfg(unix)]
    fn board_binding(done_closes_issue: bool) -> BoardBinding {
        BoardBinding {
            project_id: "PVT_1".into(),
            status_field_id: "PVTSSF_1".into(),
            status_mapping: StatusMapping {
                todo: "opt-todo".into(),
                in_progress: "opt-inprogress".into(),
                in_review: "opt-inreview".into(),
                ready_to_test: "opt-rtt".into(),
                done: "opt-done".into(),
                blocked: "opt-blocked".into(),
            },
            done_closes_issue,
            project_title: None,
            project_owner: None,
            project_owner_type: None,
            project_number: None,
            option_names: None,
        }
    }

    /// Fake `gh` for the board writes: logs every argv line, answers each
    /// GraphQL operation with canned JSON (switching on the query content),
    /// and answers the `issue view` probe with `state`. When `stale_item` is
    /// set, the option write FAILS for that item id — the stale-cache fixture.
    #[cfg(unix)]
    fn write_board_fake_gh(
        dir: &Path,
        log: &Path,
        state: &str,
        item_id: &str,
        stale_item: Option<&str>,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let node_body = json!({"data": {"repository": {"issue": {"id": "I_node"}}}}).to_string();
        let add_body =
            json!({"data": {"addProjectV2ItemById": {"item": {"id": item_id}}}}).to_string();
        let update_body =
            json!({"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": item_id}}}})
                .to_string();
        let stale_case = stale_item
            .map(|stale| {
                let stale_body =
                    json!({"errors": [{"type": "NOT_FOUND", "message": "item was removed"}]})
                        .to_string();
                format!("  *\"item={stale}\"*) printf '%s\\n' '{stale_body}'; exit 1 ;;\n")
            })
            .unwrap_or_default();
        let script = dir.join("gh-fake");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> \"{log}\"\n\
                 if [ \"$1\" = \"issue\" ]; then\n\
                 \x20 if [ \"$2\" = \"view\" ]; then printf '%s\\n' '{state}'; fi\n\
                 \x20 exit 0\n\
                 fi\n\
                 case \"$*\" in\n\
                 {stale_case}\
                 \x20 *updateProjectV2ItemFieldValue*) printf '%s\\n' '{update_body}' ;;\n\
                 \x20 *addProjectV2ItemById*) printf '%s\\n' '{add_body}' ;;\n\
                 \x20 *repository*) printf '%s\\n' '{node_body}' ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// §4.2 cold path: node-id query → addItem → option write, in that order,
    /// with the mapped OPTION ID riding the update. RTT never probes issue
    /// state, so the log is pure GraphQL.
    #[cfg(unix)]
    #[tokio::test]
    async fn board_write_with_fake_gh_cold_is_three_graphql_calls() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "OPEN", "PVTI_1", None);
        board_write_with(
            gh.to_str().unwrap(),
            &board_binding(true),
            "acme/cold",
            "7",
            BoardPhase::ReadyToTest,
        )
        .await
        .unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 3, "cold = node-id, addItem, update: {calls}");
        assert!(
            lines[0].contains("repository(owner: $owner"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("-f owner=acme")
                && lines[0].contains("-f name=cold")
                && lines[0].contains("-F number=7"),
            "slug/number become query vars: {}",
            lines[0]
        );
        assert!(lines[1].contains("addProjectV2ItemById"), "{}", lines[1]);
        assert!(
            lines[1].contains("-f content=I_node"),
            "feeds the resolved node id: {}",
            lines[1]
        );
        assert!(
            lines[2].contains("updateProjectV2ItemFieldValue"),
            "{}",
            lines[2]
        );
        assert!(
            lines[2].contains("-f item=PVTI_1"),
            "feeds the returned item id: {}",
            lines[2]
        );
        assert!(
            lines[2].contains("-f option=opt-rtt"),
            "the OPTION ID rides the write: {}",
            lines[2]
        );
    }

    /// §7.3: the second transition for an issue reuses the cached ids — ONE
    /// update call, no re-resolve (4 gh calls total across two writes; the
    /// ≤ ~10-per-run ceiling depends on this).
    #[cfg(unix)]
    #[tokio::test]
    async fn board_write_second_call_hits_cache_one_call() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "OPEN", "PVTI_2", None);
        let binding = board_binding(true);
        let program = gh.to_str().unwrap();
        board_write_with(program, &binding, "acme/warm", "8", BoardPhase::Todo)
            .await
            .unwrap();
        board_write_with(program, &binding, "acme/warm", "8", BoardPhase::ReadyToTest)
            .await
            .unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 4, "3 cold + 1 warm: {calls}");
        assert!(
            lines[3].contains("updateProjectV2ItemFieldValue")
                && lines[3].contains("-f option=opt-rtt"),
            "warm write is the update only: {}",
            lines[3]
        );
    }

    /// §4.2 step 5: an option-write failure against a CACHED item id (card
    /// removed from the board) invalidates the entry and retries ONCE cold —
    /// correctness never depends on the cache.
    #[cfg(unix)]
    #[tokio::test]
    async fn board_write_invalidates_stale_item_and_retries_once_cold() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "OPEN", "PVTI_fresh", Some("PVTI_stale"));
        // Seed the cache with a dead item id.
        ID_CACHE.lock().unwrap().insert(
            cache_key("acme/stale", "9"),
            ("I_node".into(), "PVTI_stale".into()),
        );
        board_write_with(
            gh.to_str().unwrap(),
            &board_binding(false),
            "acme/stale",
            "9",
            BoardPhase::Todo,
        )
        .await
        .unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(
            lines.len(),
            4,
            "stale update → node-id → addItem → fresh update: {calls}"
        );
        assert!(
            lines[0].contains("updateProjectV2ItemFieldValue")
                && lines[0].contains("-f item=PVTI_stale"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("repository(owner: $owner"),
            "{}",
            lines[1]
        );
        assert!(lines[2].contains("addProjectV2ItemById"), "{}", lines[2]);
        assert!(
            lines[3].contains("updateProjectV2ItemFieldValue")
                && lines[3].contains("-f item=PVTI_fresh"),
            "{}",
            lines[3]
        );
        // The heal re-populated the cache with the fresh item id.
        assert_eq!(
            ID_CACHE
                .lock()
                .unwrap()
                .get(&cache_key("acme/stale", "9"))
                .map(|(_, item)| item.clone()),
            Some("PVTI_fresh".to_string())
        );
    }

    /// D1/AC 6 close half: Done + knob ON probes state and closes an OPEN
    /// issue; an already-CLOSED issue is probed but never re-closed (the
    /// symmetric probe silences that exit-nonzero noise, §7.4).
    #[cfg(unix)]
    #[tokio::test]
    async fn done_closes_open_issue_and_skips_closed() {
        // OPEN → probe + close.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "OPEN", "PVTI_3", None);
        board_write_with(
            gh.to_str().unwrap(),
            &board_binding(true),
            "acme/close-open",
            "10",
            BoardPhase::Done,
        )
        .await
        .unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 5, "3 GraphQL + probe + close: {calls}");
        assert_eq!(
            lines[3],
            "issue view 10 --repo acme/close-open --json state --jq .state"
        );
        assert_eq!(lines[4], "issue close 10 --repo acme/close-open");

        // CLOSED → probe only, no close.
        let dir2 = tempfile::tempdir().unwrap();
        let log2 = dir2.path().join("calls.log");
        let gh2 = write_board_fake_gh(dir2.path(), &log2, "CLOSED", "PVTI_4", None);
        board_write_with(
            gh2.to_str().unwrap(),
            &board_binding(true),
            "acme/close-closed",
            "11",
            BoardPhase::Done,
        )
        .await
        .unwrap();
        let calls = std::fs::read_to_string(&log2).unwrap();
        assert_eq!(calls.lines().count(), 4, "3 GraphQL + probe: {calls}");
        assert!(!calls.contains("issue close"), "already closed: {calls}");
    }

    /// D1/AC 6 reopen half: InProgress + knob ON reopens a CLOSED issue only —
    /// an ordinary open-issue InProgress probes and does nothing (a blind
    /// reopen would exit non-zero on every one of those).
    #[cfg(unix)]
    #[tokio::test]
    async fn in_progress_reopens_closed_only() {
        // CLOSED → probe + reopen.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "CLOSED", "PVTI_5", None);
        board_write_with(
            gh.to_str().unwrap(),
            &board_binding(true),
            "acme/reopen-closed",
            "12",
            BoardPhase::InProgress,
        )
        .await
        .unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 5, "3 GraphQL + probe + reopen: {calls}");
        assert_eq!(lines[4], "issue reopen 12 --repo acme/reopen-closed");

        // OPEN → probe only, no reopen.
        let dir2 = tempfile::tempdir().unwrap();
        let log2 = dir2.path().join("calls.log");
        let gh2 = write_board_fake_gh(dir2.path(), &log2, "OPEN", "PVTI_6", None);
        board_write_with(
            gh2.to_str().unwrap(),
            &board_binding(true),
            "acme/reopen-open",
            "13",
            BoardPhase::InProgress,
        )
        .await
        .unwrap();
        let calls = std::fs::read_to_string(&log2).unwrap();
        assert_eq!(calls.lines().count(), 4, "3 GraphQL + probe: {calls}");
        assert!(!calls.contains("issue reopen"), "already open: {calls}");
    }

    /// D1's OFF side: with `done_closes_issue: false` NEITHER direction even
    /// probes — we never closed, so we never reopen; a human-closed issue on a
    /// knob-off binding is respected.
    #[cfg(unix)]
    #[tokio::test]
    async fn knob_off_never_probes_closes_or_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let gh = write_board_fake_gh(dir.path(), &log, "OPEN", "PVTI_7", None);
        let binding = board_binding(false);
        let program = gh.to_str().unwrap();
        board_write_with(program, &binding, "acme/knob-off", "14", BoardPhase::Done)
            .await
            .unwrap();
        board_write_with(
            program,
            &binding,
            "acme/knob-off",
            "14",
            BoardPhase::InProgress,
        )
        .await
        .unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            calls.lines().count(),
            4,
            "3 cold + 1 warm, pure GraphQL: {calls}"
        );
        assert!(
            !calls.contains("issue view")
                && !calls.contains("issue close")
                && !calls.contains("issue reopen"),
            "knob OFF must never probe/close/reopen: {calls}"
        );
    }
}
