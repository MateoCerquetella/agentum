//! Provider-agnostic task destination ("sink") for the chat-to-features
//! pipeline (spec 011).
//!
//! A user describes work in the planner "New Goal" entry; the planner
//! decomposes it into features; those features are *created* in whatever task
//! manager the user has configured. This module is the seam that hides which
//! manager that is — the rest of the pipeline calls [`TaskSink::create_feature`]
//! and never names a provider.
//!
//! Modelled as a **closed enum**, not a plugin trait, to match the codebase's
//! seam style (cf. `mcp_provision::BrowserMcpEngine`): every supported provider
//! is visible in one `match`, there's no dynamic dispatch, and no new
//! dependency (`async-trait`). v1 ships the internal board (011a) and GitHub
//! Issues (011b); Linear (011c) slots in as a new variant.

use std::path::{Path, PathBuf};

use agentum_core::NewBoardItem;
use agentum_store::Store;

/// A feature to create in a task destination. The minimal shape every provider
/// can accept — title is required, body optional.
#[derive(Debug, Clone)]
pub struct NewFeature {
    pub title: String,
    pub body: Option<String>,
    /// Labels to apply on creation. GitHub passes each as `--label <l>`; the
    /// board and Linear arms currently ignore them (Linear label application is a
    /// documented v1 no-op — see `routes::chat`). Empty = no labels, so existing
    /// callers stay byte-for-byte unchanged.
    pub labels: Vec<String>,
}

/// Where a created feature landed. `id` is the provider's stable handle (board
/// key like `AG-12`, a GitHub issue number, a Linear identifier) — the
/// chat-to-features pipeline reuses it as the harness feature id so
/// `$HARNESS_FEATURE_ID` in `verify.sh` points back at the real tracker item.
///
/// `Serialize` so the `/api/board/goals` handler can return it verbatim as the
/// Chat-create response (spec 018) — `provider`/`id`/`url` are the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FeatureRef {
    pub provider: &'static str,
    pub id: String,
    pub url: Option<String>,
}

/// What a sink needs to create a feature. Board uses the store + parent goal;
/// CLI-backed providers (GitHub `gh`) run inside the repo at `workdir`.
pub struct SinkCtx<'a> {
    pub store: &'a Store,
    /// Repo directory for CLI-based providers; also where the goal lives.
    pub workdir: &'a Path,
    /// Parent goal id for hierarchy-aware providers (the board nests under it).
    pub parent_goal_id: Option<i64>,
    /// Explicit GitHub `owner/repo` target (spec 019). When `Some`, the GitHub
    /// arm files via `gh issue create --repo <slug>` run from `$HOME` — so a
    /// non-existent project `workdir` is never used as cwd (the Chat-from-
    /// anywhere fix). When `None`, the legacy cwd-relative argv runs inside
    /// `workdir` (harness/`plan_goal_harness` compatibility — byte-for-byte
    /// unchanged).
    pub slug: Option<&'a str>,
}

/// The configured task destination. The internal board is also the agnostic
/// *fallback* — it is the source of truth whenever no external manager is
/// connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSink {
    /// agentum's own kanban board (`board_items`).
    Board,
    /// GitHub Issues via the authenticated `gh` CLI (spec 011b).
    Github,
    /// Linear via the GraphQL `issueCreate` mutation (spec 011c).
    Linear,
}

impl TaskSink {
    /// Stable provider id, surfaced to callers and stamped onto [`FeatureRef`].
    pub fn provider(self) -> &'static str {
        match self {
            TaskSink::Board => "board",
            TaskSink::Github => "github",
            TaskSink::Linear => "linear",
        }
    }

    /// Decide the destination from which providers are available. Pure policy so
    /// it's unit-testable; the IO that discovers availability lives in
    /// [`TaskSink::select`].
    ///
    /// An external manager is the source of truth when configured; the internal
    /// board is the agnostic fallback. GitHub takes precedence over Linear when
    /// both are present (deterministic + documented).
    pub fn pick_provider(github_available: bool, linear_available: bool) -> TaskSink {
        if github_available {
            TaskSink::Github
        } else if linear_available {
            TaskSink::Linear
        } else {
            TaskSink::Board
        }
    }

    /// Resolve the destination for a goal's `workdir` by probing what's
    /// configured, then delegating to [`TaskSink::pick_provider`].
    ///
    /// `AGENTUM_TASK_SINK=board|github|linear` forces a provider, overriding
    /// detection — useful to pin a destination (and to keep tests hermetic).
    pub async fn select(workdir: &Path) -> TaskSink {
        match std::env::var("AGENTUM_TASK_SINK").as_deref() {
            Ok("board") => return TaskSink::Board,
            Ok("github") => return TaskSink::Github,
            Ok("linear") => return TaskSink::Linear,
            _ => {}
        }
        TaskSink::pick_provider(github_ready(workdir).await, crate::linear::available())
    }

    /// Create one feature in the backing task manager. Returns a [`FeatureRef`]
    /// the caller can surface; an `Err` must be propagated, never swallowed —
    /// the pipeline reports per-feature failures rather than silently dropping
    /// them (spec 011 risk: no silent partial state).
    pub async fn create_feature(
        self,
        ctx: &SinkCtx<'_>,
        feature: &NewFeature,
    ) -> anyhow::Result<FeatureRef> {
        match self {
            TaskSink::Board => {
                // A feature is a `feat` card in `todo`; the board's `todo` gate
                // requires only Title + Lbl, both present here. The card mirrors
                // the feature on the kanban view; when later moved to `doing` the
                // existing board flow spawns its agent session.
                let item = ctx
                    .store
                    .create_board_item(NewBoardItem {
                        title: feature.title.clone(),
                        body: feature.body.clone(),
                        lbl: Some("feat".into()),
                        status: Some("todo".into()),
                        workdir: None,
                        parent_goal_id: ctx.parent_goal_id,
                        tool: None,
                        model: None,
                        session_id: None,
                        priority: None,
                    })
                    .await?;
                Ok(FeatureRef {
                    provider: self.provider(),
                    id: item.key,
                    url: None,
                })
            }
            TaskSink::Github => {
                // Non-interactive create: with both --title and --body present,
                // `gh` skips its editor and prints the new issue URL to stdout.
                // `gh_bin()` (not a literal "gh") so the create arm honors the
                // same AGENTUM_GH_BIN knob the transition arm already does —
                // which is also what lets the spec 006 fake-gh wire test pin
                // the full plan → argv chain (Mateo's empty-body report).
                let body = feature.body.clone().unwrap_or_default();
                let mut cmd = tokio::process::Command::new(gh_bin());
                match ctx.slug {
                    // Spec 019: an explicit `--repo owner/repo` target makes `gh`
                    // ignore the cwd's git remote, so we run from a neutral,
                    // always-present dir ($HOME) — a missing/remote project
                    // workdir is never used as cwd.
                    Some(slug) => {
                        cmd.args(gh_create_argv_with_repo(
                            slug,
                            &feature.title,
                            &body,
                            &feature.labels,
                        ))
                        .current_dir(neutral_cwd());
                    }
                    // Legacy harness path: resolve the repo from `workdir`'s
                    // origin (cwd-relative). Unchanged behavior for callers
                    // (e.g. `plan_goal_harness`) that pass no slug.
                    None => {
                        cmd.args(gh_create_argv(&feature.title, &body, &feature.labels))
                            .current_dir(ctx.workdir);
                    }
                }
                let output = cmd
                    .output()
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to run `gh`: {e}"))?;
                if !output.status.success() {
                    anyhow::bail!(
                        "gh issue create failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                parse_gh_issue_url(&String::from_utf8_lossy(&output.stdout))
            }
            TaskSink::Linear => {
                let (id, url) = crate::linear::create_issue(
                    &feature.title,
                    feature.body.as_deref().unwrap_or_default(),
                )
                .await?;
                Ok(FeatureRef {
                    provider: self.provider(),
                    id,
                    url,
                })
            }
        }
    }
}

/// A pipeline phase, mapped onto whatever the backing tracker calls it. The
/// harness drives these as a feature moves Pending → Coding → green unit gate →
/// green QA gate (spec 012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerPhase {
    Todo,
    InProgress,
    /// Spec 012 F3: a PR is open on the workspace's branch. Sits between
    /// InProgress and ReadyToTest in the canonical order (`tracker_sync`), so a
    /// plain session walks InProgress → InReview → Done while a gated run's
    /// ReadyToTest (unit-green) never regresses when a PR opens.
    InReview,
    ReadyToTest,
    Done,
}

impl TrackerPhase {
    /// The lowercase wire form (the exact inverse of [`parse_tracker_phase`]).
    /// Lives on the seam type (spec 014 F1) so the emitted
    /// `tracker.phase_changed` payload and the persisted `tracker_phase` can
    /// never drift; `tracker_sync::tracker_phase_wire` delegates here.
    pub(crate) fn wire_str(self) -> &'static str {
        match self {
            TrackerPhase::Todo => "todo",
            TrackerPhase::InProgress => "in_progress",
            TrackerPhase::InReview => "in_review",
            TrackerPhase::ReadyToTest => "ready_to_test",
            TrackerPhase::Done => "done",
        }
    }
}

/// Parse a wire-format phase string (`todo` / `in_progress` / `in_review` /
/// `ready_to_test` / `done`) into a [`TrackerPhase`]. Pure; `None` for anything
/// else — the MCP `agentum_report_status` tool (spec 005 F4) treats that as a
/// caller bug, not a tracker hiccup.
pub fn parse_tracker_phase(s: &str) -> Option<TrackerPhase> {
    match s {
        "todo" => Some(TrackerPhase::Todo),
        "in_progress" => Some(TrackerPhase::InProgress),
        "in_review" => Some(TrackerPhase::InReview),
        "ready_to_test" => Some(TrackerPhase::ReadyToTest),
        "done" => Some(TrackerPhase::Done),
        _ => None,
    }
}

/// Outcome of a transition, for the harness log. Transitions are a side-channel:
/// a tracker hiccup must never halt the run, so even failures come back as a
/// value the caller logs rather than an error that propagates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionResult {
    /// The tracker state was changed.
    Applied,
    /// Not fully applied — the reason names what did and didn't land. Covers
    /// both "nothing to do" (provider has no such concept, or no external
    /// tracker) and, since spec 010 F2, a partial write on a board-bound repo
    /// (e.g. "status label applied; Projects board write failed: …").
    Skipped(String),
}

/// The board column for a phase. Pure so the mapping is unit-tested. The board
/// ships `todo`/`doing`/`review`/`done` columns (see board_column_rule tests).
fn board_status_for(phase: TrackerPhase) -> &'static str {
    match phase {
        TrackerPhase::Todo => "todo",
        TrackerPhase::InProgress => "doing",
        // The internal board has no distinct in-review column — InReview folds
        // onto `review` alongside ReadyToTest (the board is a coarse mirror).
        TrackerPhase::InReview => "review",
        TrackerPhase::ReadyToTest => "review",
        TrackerPhase::Done => "done",
    }
}

/// Canonical, harness-owned status labels with fixed colors (spec 004 D3).
/// NOT `.github/labels.sh`'s `status/qa*` set — that is the human-QA lifecycle
/// (architecture C4); the transition never touches foreign `status/*` labels.
const GITHUB_STATUS_LABELS: [(TrackerPhase, &str, &str); 5] = [
    (TrackerPhase::Todo, "status/todo", "ededed"),
    (TrackerPhase::InProgress, "status/in-progress", "1d76db"),
    // Spec 012 F3: the fifth mutually-exclusive pipeline label (purple).
    (TrackerPhase::InReview, "status/in-review", "5319e7"),
    (TrackerPhase::ReadyToTest, "status/ready-to-test", "fbca04"),
    (TrackerPhase::Done, "status/done", "0e8a16"),
];

/// The escalation label for a feature that exhausted its retries (spec 008 D6).
/// FIXED (not configurable via [`GithubStateMap`]): it is not a pipeline phase —
/// Linear/board have no "blocked" column — so it stays out of `TrackerPhase`
/// (D-A) and lives only on the GitHub-label layer. Red (`b60205`).
const GITHUB_BLOCKED_LABEL: (&str, &str) = ("status/blocked", "b60205");

/// All SIX canonical status names for the one-per-issue remove-set (spec 008
/// D6, extended spec 012 F3): the five CONFIGURED pipeline names + the fixed
/// blocked name. Every
/// pipeline transition removes this whole set (minus its own target), so setting
/// any pipeline label also clears `status/blocked` — the board can't lie in
/// either direction (a re-driven blocked feature drops the label at InProgress).
/// Callers dedupe against the target so a name-collision can't remove the target.
fn all_status_label_names(map: &GithubStateMap) -> Vec<&str> {
    let mut names: Vec<&str> = map.labels().to_vec();
    names.push(GITHUB_BLOCKED_LABEL.0);
    names
}

/// The canonical (default) GitHub label for a phase. Stays the default-name
/// accessor after spec 005 F5 made names configurable: [`GithubStateMap`]'s
/// `Default` delegates here so the two can never drift.
fn github_status_label(phase: TrackerPhase) -> &'static str {
    GITHUB_STATUS_LABELS
        .iter()
        .find(|(p, _, _)| *p == phase)
        .map(|(_, name, _)| *name)
        .expect("GITHUB_STATUS_LABELS covers every TrackerPhase")
}

/// The canonical ensure-create color for a phase. Colors key off the PHASE,
/// never the label name (spec 005 F5): a custom-named label inherits its
/// phase's canonical color via the same `--force` ensure-create, so a renamed
/// pipeline still reads at a glance and a manually recolored label self-heals.
/// `pub(crate)` for spec 010 F3: provisioning runs its OWN label-ensure loop
/// over this builder pair (never a refactor of the transition's pinned
/// ensure sequence).
pub(crate) fn github_status_color(phase: TrackerPhase) -> &'static str {
    GITHUB_STATUS_LABELS
        .iter()
        .find(|(p, _, _)| *p == phase)
        .map(|(_, _, color)| *color)
        .expect("GITHUB_STATUS_LABELS covers every TrackerPhase")
}

/// The four pipeline phases → GitHub *label names* (spec 005 F5, D4). Teams
/// with their own status vocabulary configure names here; the transport
/// (ensure-create + one edit) is unchanged from spec 004.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubStateMap {
    pub todo: String,
    pub in_progress: String,
    pub in_review: String,
    pub ready_to_test: String,
    pub done: String,
}

impl Default for GithubStateMap {
    /// The canonical five from [`GITHUB_STATUS_LABELS`], via the
    /// [`github_status_label`] accessor so defaults and table can't drift.
    fn default() -> Self {
        Self {
            todo: github_status_label(TrackerPhase::Todo).into(),
            in_progress: github_status_label(TrackerPhase::InProgress).into(),
            in_review: github_status_label(TrackerPhase::InReview).into(),
            ready_to_test: github_status_label(TrackerPhase::ReadyToTest).into(),
            done: github_status_label(TrackerPhase::Done).into(),
        }
    }
}

/// The persisted label-name overrides (Settings → Integrations → GitHub).
/// Each field optional so a partial override keeps the default for the rest.
/// Field names match the desktop's `commands/github_labels.rs` exactly — the
/// server reads the same `github.json` the desktop writes.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct StoredGithubStateMap {
    #[serde(default)]
    todo: Option<String>,
    #[serde(default)]
    in_progress: Option<String>,
    #[serde(default)]
    in_review: Option<String>,
    #[serde(default)]
    ready_to_test: Option<String>,
    #[serde(default)]
    done: Option<String>,
}

/// `github.json` — the desktop-owned GitHub pipeline config (the `linear.json`
/// sibling). Only `state_map` today.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct GithubConfigFile {
    #[serde(default)]
    state_map: Option<StoredGithubStateMap>,
}

/// Path to the desktop's GitHub pipeline config. Mirrors
/// `linear.rs::creds_path` exactly (`<data_local_dir|data_dir>/Agentum/
/// github.json`) so the server reads the same file the desktop Settings pane
/// writes. `AGENTUM_GITHUB_CONFIG` overrides it (tests/CI — mirrors
/// `AGENTUM_LINEAR_CREDS`).
fn github_config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AGENTUM_GITHUB_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = dirs::data_local_dir().or_else(dirs::data_dir)?;
    Some(base.join("Agentum").join("github.json"))
}

/// Absent/unreadable/garbled → `Default` (no overrides), never an error — the
/// map must resolve even on a machine with no desktop config.
fn read_github_config() -> GithubConfigFile {
    github_config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

impl GithubStateMap {
    /// Resolve the effective label map: defaults → `github.json` `state_map`
    /// (written by Settings) → `AGENTUM_GITHUB_STATUS_{TODO,IN_PROGRESS,
    /// READY_TO_TEST,DONE}` env (highest precedence, for tests/CI). A partial
    /// override at any layer keeps the lower layer's value — byte-for-byte the
    /// `LinearStateMap::from_env` layering.
    pub fn from_env() -> Self {
        Self::apply_layers(read_github_config().state_map, |k| std::env::var(k).ok())
    }

    /// Pure layering core: precedence is tested by injecting the file shape
    /// and an env closure — never by mutating process env (parallel tests).
    /// Blank/whitespace values at any layer keep the lower layer's value.
    fn apply_layers(
        file: Option<StoredGithubStateMap>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut m = Self::default();
        // Layer 1: persisted Settings overrides from github.json.
        if let Some(sm) = file {
            for (slot, v) in [
                (&mut m.todo, sm.todo),
                (&mut m.in_progress, sm.in_progress),
                (&mut m.in_review, sm.in_review),
                (&mut m.ready_to_test, sm.ready_to_test),
                (&mut m.done, sm.done),
            ] {
                if let Some(v) = v.filter(|s| !s.trim().is_empty()) {
                    *slot = v.trim().to_string();
                }
            }
        }
        // Layer 2: env overrides (win over the file).
        for (slot, key) in [
            (&mut m.todo, "AGENTUM_GITHUB_STATUS_TODO"),
            (&mut m.in_progress, "AGENTUM_GITHUB_STATUS_IN_PROGRESS"),
            (&mut m.in_review, "AGENTUM_GITHUB_STATUS_IN_REVIEW"),
            (&mut m.ready_to_test, "AGENTUM_GITHUB_STATUS_READY_TO_TEST"),
            (&mut m.done, "AGENTUM_GITHUB_STATUS_DONE"),
        ] {
            if let Some(v) = env(key).filter(|s| !s.trim().is_empty()) {
                *slot = v.trim().to_string();
            }
        }
        m
    }

    /// The configured label name for a pipeline phase.
    pub fn label_for(&self, phase: TrackerPhase) -> &str {
        match phase {
            TrackerPhase::Todo => &self.todo,
            TrackerPhase::InProgress => &self.in_progress,
            TrackerPhase::InReview => &self.in_review,
            TrackerPhase::ReadyToTest => &self.ready_to_test,
            TrackerPhase::Done => &self.done,
        }
    }

    /// The configured names in canonical phase order (may contain duplicates
    /// when a user maps two phases to one name — callers dedupe by name).
    fn labels(&self) -> [&str; 5] {
        [
            &self.todo,
            &self.in_progress,
            &self.in_review,
            &self.ready_to_test,
            &self.done,
        ]
    }
}

/// Idempotent ensure-create: `--force` updates an existing label's color to
/// canonical instead of failing. One argv token per value — never a shell.
/// `pub(crate)` for spec 010 F3 (see [`github_status_color`]).
pub(crate) fn gh_label_ensure_argv<'a>(
    name: &'a str,
    slug: &'a str,
    color: &'a str,
) -> [&'a str; 8] {
    [
        "label", "create", name, "--repo", slug, "--color", color, "--force",
    ]
}

/// Set-one/remove-others in ONE `gh issue edit`, against the CONFIGURED label
/// set (spec 005 F5): add the target label, then deterministically remove the
/// other configured names (no read-modify-write; `gh` treats removing an
/// absent label as a no-op). The remove-filter is by NAME, not phase: if a
/// user maps two phases to one name, the target never appears in its own
/// remove list; duplicate names are deduped. By construction the argv can only
/// name configured labels, so foreign `status/*` labels — `status/qa*` (C4),
/// or a label applied under an OLDER map whose name is no longer configured —
/// are never touched. (Removing a stale-map label would require
/// read-modify-write over arbitrary `status/*` names, exactly the
/// foreign-label hazard the deterministic remove-set exists to prevent.)
fn gh_set_status_label_argv<'a>(
    number: &'a str,
    slug: &'a str,
    phase: TrackerPhase,
    map: &'a GithubStateMap,
) -> Vec<&'a str> {
    let target = map.label_for(phase);
    let mut argv = vec![
        "issue",
        "edit",
        number,
        "--repo",
        slug,
        "--add-label",
        target,
    ];
    // Remove the OTHER four of the five canonical names (3 pipeline + blocked),
    // deduped by name (spec 008 D6): removing an absent label is a `gh` no-op, so
    // a pipeline flip also clears any lingering `status/blocked` for free.
    let mut removed: Vec<&str> = Vec::new();
    for name in all_status_label_names(map) {
        if name != target && !removed.contains(&name) {
            removed.push(name);
            argv.push("--remove-label");
            argv.push(name);
        }
    }
    argv
}

/// Blocked → `status/blocked`, removing the four configured pipeline names.
/// The mirror of [`gh_set_status_label_argv`] with the target fixed to the
/// blocked label (spec 008 D6). Deduped by name; the target (blocked) is never
/// in `map.labels()` on the happy path, so all four pipeline names are removed.
fn gh_set_blocked_label_argv<'a>(
    number: &'a str,
    slug: &'a str,
    map: &'a GithubStateMap,
) -> Vec<&'a str> {
    let target = GITHUB_BLOCKED_LABEL.0;
    let mut argv = vec![
        "issue",
        "edit",
        number,
        "--repo",
        slug,
        "--add-label",
        target,
    ];
    let mut removed: Vec<&str> = Vec::new();
    for name in map.labels() {
        if name != target && !removed.contains(&name) {
            removed.push(name);
            argv.push("--remove-label");
            argv.push(name);
        }
    }
    argv
}

/// `gh issue comment` argv (spec 008 D6). Pure — the body is a single argv token
/// (`--body <body>`), never shell-interpolated, so a multi-line comment with
/// backticks/newlines is passed to `gh` verbatim and can't inject a flag.
fn gh_issue_comment_argv<'a>(number: &'a str, slug: &'a str, body: &'a str) -> [&'a str; 7] {
    ["issue", "comment", number, "--repo", slug, "--body", body]
}

/// The AC-4 blocked comment: the retry count + the gate-output tail, wrapped in a
/// GitHub-collapsible `<details>` so a long tail doesn't dominate the thread.
fn blocked_comment_body(
    feature_name: &str,
    gate_label: &str,
    attempts: u32,
    gate_tail: &str,
) -> String {
    format!(
        "⛔ **Blocked** — `{feature_name}` failed the {gate_label} after {attempts} attempt(s).\n\n\
         <details><summary>Gate output (tail)</summary>\n\n```\n{gate_tail}\n```\n</details>\n\n\
         _Posted by the agentum Harness Engine._"
    )
}

/// `https://github.com/{owner}/{repo}/issues/{n}` → `(slug, number)`. Tolerates
/// a trailing slash and a query/fragment; rejects `/pull/` URLs, non-github
/// hosts, and non-numeric tails. Pure. Lives here (not `board_sync`) because
/// `task_sink` is a crate-root seam and must not depend on a route module.
/// `pub(crate)` so the MCP `agentum_report_status` tool (spec 005 F4) can
/// derive a missing GitHub `id` from the ticket URL with the same parser.
pub(crate) fn github_slug_and_number_from_issue_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    let kind = parts.next()?;
    let number = parts.next()?;
    if kind != "issues" || parts.next().is_some() {
        return None;
    }
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((format!("{owner}/{repo}"), number.to_string()))
}

/// `gh` binary override — a real knob (a server with `gh` off PATH), and the
/// docs hook for tests (which pass the program explicitly; no env mutation).
fn gh_bin() -> String {
    std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into())
}

/// One `gh` call from `neutral_cwd()` — the same cwd discipline creation uses
/// (an explicit `--repo` makes the cwd's git remote irrelevant). Ok on exit 0;
/// Err carries stderr truncated to ~240 chars. Bounded by a 30s timeout so a
/// hung `gh` (network stall) degrades to a `Skipped`, never a stalled run.
async fn run_gh(program: &str, args: &[&str]) -> Result<(), String> {
    let fut = tokio::process::Command::new(program)
        .args(args)
        .current_dir(neutral_cwd())
        .output();
    let output = match tokio::time::timeout(std::time::Duration::from_secs(30), fut).await {
        Err(_) => return Err("gh timed out".into()),
        Ok(Err(e)) => return Err(format!("failed to run `{program}`: {e}")),
        Ok(Ok(o)) => o,
    };
    if output.status.success() {
        return Ok(());
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

/// Ensure the 4 configured labels exist (each failure NON-fatal — a shared
/// repo without label-create permission can still add an existing label), then
/// the single `issue edit` decides: exit 0 → `Applied`; anything else →
/// `Skipped(reason)`. Never returns `Err` — tracker sync is best-effort (AC 5).
/// `program` is explicit so tests inject a fake `gh` without env mutation;
/// `map` is explicit for the same reason (spec 005 F5). Colors key off the
/// PHASE ([`github_status_color`]); a duplicate name is ensured once — the
/// first phase in canonical order wins its color.
async fn github_transition_with(
    program: &str,
    slug: &str,
    number: &str,
    phase: TrackerPhase,
    map: &GithubStateMap,
) -> TransitionResult {
    let mut ensured: Vec<&str> = Vec::new();
    for (p, _, _) in GITHUB_STATUS_LABELS.iter() {
        let name = map.label_for(*p);
        if ensured.contains(&name) {
            continue;
        }
        ensured.push(name);
        let _ = run_gh(
            program,
            &gh_label_ensure_argv(name, slug, github_status_color(*p)),
        )
        .await;
    }
    match run_gh(program, &gh_set_status_label_argv(number, slug, phase, map)).await {
        Ok(()) => TransitionResult::Applied,
        Err(reason) => TransitionResult::Skipped(reason),
    }
}

/// Label transition + (when bound) the ADDITIVE Projects-board write (spec 010
/// F2). The label path is the byte-identical [`github_transition_with`]
/// (AC 8); unbound (`binding: None`) returns its result untouched — today's
/// behavior byte-for-byte. A board failure can only append to the report,
/// never alter label behavior, and NEVER becomes an `Err` (AC 7): it folds
/// into the existing `Skipped(reason)` so `drive.rs`'s `transition_tracker`
/// log line and the MCP tool's `skipped:` text carry it with zero call-site
/// edits — loud through today's plumbing (§7.9).
async fn github_transition_with_board(
    program: &str,
    slug: &str,
    number: &str,
    phase: TrackerPhase,
    map: &GithubStateMap,
    binding: Option<&crate::github_projects::BoardBinding>,
) -> TransitionResult {
    let label = github_transition_with(program, slug, number, phase, map).await;
    let Some(b) = binding else { return label };
    match crate::github_projects::board_write_with(program, b, slug, number, phase.into()).await {
        Ok(()) => label,
        Err(reason) => {
            tracing::warn!(slug, number, ?phase, %reason, "Projects board write failed (non-fatal)");
            match label {
                TransitionResult::Applied => TransitionResult::Skipped(format!(
                    "status label applied; Projects board write failed: {reason}"
                )),
                TransitionResult::Skipped(why) => TransitionResult::Skipped(format!(
                    "{why}; Projects board write failed: {reason}"
                )),
            }
        }
    }
}

/// GitHub block escalation (spec 008 D6): ensure `status/blocked` exists, one
/// `issue edit` (add blocked + remove the four pipeline names), then one
/// best-effort comment (retry count + gate tail). `Applied` iff the LABEL edit
/// succeeds — the comment is secondary, so its failure is dropped, never a
/// downgrade to `Skipped`. `program`/`map` are explicit so the fake-`gh` test
/// injects them without env mutation (mirrors [`github_transition_with`]).
///
/// `with_comment: false` (spec 014 F4, the crash-loop guard) skips ONLY the
/// comment step — the idempotent label edit still runs, so a re-blocked issue
/// keeps its flag without a duplicate comment inside the cooldown.
#[allow(clippy::too_many_arguments)]
async fn github_mark_blocked_with(
    program: &str,
    slug: &str,
    number: &str,
    feature_name: &str,
    gate_label: &str,
    attempts: u32,
    gate_tail: &str,
    with_comment: bool,
    map: &GithubStateMap,
) -> TransitionResult {
    // Ensure the blocked label exists (non-fatal — a shared repo without
    // label-create permission can still add an existing label).
    let _ = run_gh(
        program,
        &gh_label_ensure_argv(GITHUB_BLOCKED_LABEL.0, slug, GITHUB_BLOCKED_LABEL.1),
    )
    .await;
    // The label edit decides Applied/Skipped.
    if let Err(reason) = run_gh(program, &gh_set_blocked_label_argv(number, slug, map)).await {
        return TransitionResult::Skipped(reason);
    }
    // Best-effort comment; a failure here never downgrades the applied label.
    if with_comment {
        let body = blocked_comment_body(feature_name, gate_label, attempts, gate_tail);
        let _ = run_gh(program, &gh_issue_comment_argv(number, slug, &body)).await;
    }
    TransitionResult::Applied
}

/// The blocked-path sibling of [`github_transition_with_board`] (spec 010
/// AC 5): today's label+comment escalation ([`github_mark_blocked_with`],
/// byte-identical) plus, when bound, the ADDITIVE card move to the
/// Blocked-mapped option. `board_write_with` never probes/closes/reopens for
/// `BoardPhase::Blocked` (that is Done/InProgress-only), and a board failure
/// folds into the returned reason exactly like the pipeline seam — never an
/// `Err`, never a change to label behavior.
#[allow(clippy::too_many_arguments)]
async fn github_mark_blocked_with_board(
    program: &str,
    slug: &str,
    number: &str,
    feature_name: &str,
    gate_label: &str,
    attempts: u32,
    gate_tail: &str,
    with_comment: bool,
    map: &GithubStateMap,
    binding: Option<&crate::github_projects::BoardBinding>,
) -> TransitionResult {
    let label = github_mark_blocked_with(
        program,
        slug,
        number,
        feature_name,
        gate_label,
        attempts,
        gate_tail,
        with_comment,
        map,
    )
    .await;
    let Some(b) = binding else { return label };
    match crate::github_projects::board_write_with(
        program,
        b,
        slug,
        number,
        crate::github_projects::BoardPhase::Blocked,
    )
    .await
    {
        Ok(()) => label,
        // The identical fold — see `github_transition_with_board`.
        Err(reason) => {
            tracing::warn!(slug, number, %reason, "Projects board write failed (non-fatal)");
            match label {
                TransitionResult::Applied => TransitionResult::Skipped(format!(
                    "status label applied; Projects board write failed: {reason}"
                )),
                TransitionResult::Skipped(why) => TransitionResult::Skipped(format!(
                    "{why}; Projects board write failed: {reason}"
                )),
            }
        }
    }
}

/// Fire-and-forget emission coords for the spec-014 tracker bus events. A
/// REQUIRED parameter on both seam fns so "transition without emitting" is
/// unrepresentable — every existing and future caller must provide a bus (to
/// skip emission you'd have to pass a dummy channel, visible in review).
pub struct TrackerEmit<'a> {
    pub bus: &'a tokio::sync::broadcast::Sender<agentum_core::Event>,
    /// The bound workspace, when the caller knows it (reactor / poller /
    /// attention worker). `None` for tracker-coord-only callers (harness,
    /// MCP, planning) — consumers then join on `tracker_url`.
    pub worktree_id: Option<&'a str>,
}

/// Drive a created feature's tracker item to `phase`, dispatching on the provider
/// recorded when the feature was created. **Best-effort by contract**: returns
/// `Ok(Skipped)` for providers/states that don't apply and only `Err` for a real
/// transport failure the caller should log — never a reason to halt the harness.
///
/// `tracker_id` is the provider's stable handle (board key, Linear identifier,
/// GitHub issue number) — the same value stored as the harness feature id.
/// `tracker_url` is the ticket's URL when known (`Feature.tracker_url`); the
/// GitHub arm parses `owner/repo` AND the issue number from it (spec 004 — a
/// spec-from-issue backlog derives N features from ONE issue, so `tracker_id`
/// cannot double as the issue number). Board/Linear ignore it.
///
/// Spec 014 F1: on — and ONLY on — `Ok(Applied)` a `tracker.phase_changed`
/// event is emitted on the bus. `broadcast::Sender::send` is synchronous and
/// non-blocking; the ignored zero-receiver `Err` makes fire-and-forget
/// structural. `Skipped`/`Err` emit nothing (the bus never lies) — including a
/// partial write that folds into `Skipped` (the persisted-phase re-fetch
/// reconciles clients).
pub async fn apply_tracker_transition(
    store: &Store,
    provider: &str,
    tracker_id: &str,
    tracker_url: Option<&str>,
    phase: TrackerPhase,
    emit: TrackerEmit<'_>,
) -> anyhow::Result<TransitionResult> {
    let result = transition_inner(store, provider, tracker_id, tracker_url, phase).await;
    if matches!(result, Ok(TransitionResult::Applied)) {
        let _ = emit.bus.send(
            agentum_core::Event::new("tracker.phase_changed").with_payload(serde_json::json!({
                "worktree_id": emit.worktree_id,
                "provider": provider,
                "phase": phase.wire_str(),
                "tracker_url": tracker_url,
            })),
        );
    }
    result
}

/// The pre-014 transition body, verbatim — the wrapper above owns emission so
/// the only-on-Applied rule is enforced at exactly one `matches!` arm.
async fn transition_inner(
    store: &Store,
    provider: &str,
    tracker_id: &str,
    tracker_url: Option<&str>,
    phase: TrackerPhase,
) -> anyhow::Result<TransitionResult> {
    match provider {
        "linear" => {
            let map = crate::linear::LinearStateMap::from_env();
            match crate::linear::transition_issue(tracker_id, phase, &map).await? {
                crate::linear::TransitionOutcome::Applied => Ok(TransitionResult::Applied),
                crate::linear::TransitionOutcome::Skipped(why) => {
                    Ok(TransitionResult::Skipped(why))
                }
            }
        }
        "board" => {
            // The board key (e.g. `AG-12`) is the feature id; resolve it to the
            // numeric row id to patch status.
            let items = store.list_board_items().await?;
            let Some(item) = items.into_iter().find(|i| i.key == tracker_id) else {
                return Ok(TransitionResult::Skipped(format!(
                    "no board card with key {tracker_id}"
                )));
            };
            let status = board_status_for(phase);
            store
                .patch_board_item(
                    item.id,
                    agentum_core::BoardPatch {
                        status: Some(status.to_string()),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(TransitionResult::Applied)
        }
        // GitHub Issues has no workflow column; the phase lives as exactly one
        // canonical `status/*` label (spec 004 D3). Unbound, `Done` is
        // label-only — the issue stays open; closing remains the PR's
        // `Closes #N` job (004 D1). On a board-BOUND repo the additive
        // Projects arm also moves the card and may close/reopen at
        // Done/InProgress (spec 010 D1 supersedes 004 D1 for bound repos only).
        "github" => {
            let Some(url) = tracker_url.map(str::trim).filter(|u| !u.is_empty()) else {
                return Ok(TransitionResult::Skipped(
                    "feature has no tracker_url; owner/repo unknown".into(),
                ));
            };
            let Some((slug, number)) = github_slug_and_number_from_issue_url(url) else {
                return Ok(TransitionResult::Skipped(format!(
                    "cannot parse a GitHub issue from {url}"
                )));
            };
            // Label names are configurable (spec 005 F5). Resolve the map only
            // AFTER the URL parse succeeds so the no-url/unparseable skips
            // never touch the config file (keeps those tests hermetic).
            let map = GithubStateMap::from_env();
            // Spec 010 F2: the binding read follows the SAME hermeticity
            // discipline — only after the parse, so the no-url/unparseable
            // skip tests never touch the config files.
            let binding = crate::github_projects::binding_for_slug(&slug);
            Ok(github_transition_with_board(
                &gh_bin(),
                &slug,
                &number,
                phase,
                &map,
                binding.as_ref(),
            )
            .await)
        }
        other => Ok(TransitionResult::Skipped(format!(
            "unknown tracker provider {other:?}"
        ))),
    }
}

/// The block-path sibling of [`apply_tracker_transition`] (spec 008 D6): when a
/// feature exhausts its retries, escalate on the ISSUE with a `status/blocked`
/// label plus a comment carrying the retry count and the gate-output tail.
/// GitHub-only: board/linear have no blocked column, so they
/// `Skipped("no blocked state")` (D-A — `TrackerPhase` stays four variants).
/// **Best-effort by contract**: like its sibling it returns only
/// `Applied`/`Skipped` and NEVER `Err` for a tracker hiccup, so `drive.rs` logs
/// the outcome and a blocked issue-update failure can never halt the
/// (already-halted) run.
///
/// `store`/`tracker_id` are accepted for signature-parity with
/// `apply_tracker_transition` (so `drive.rs` calls both identically) but unused
/// while blocked is GitHub-only — the GitHub arm derives owner/repo + number from
/// `tracker_url`, and board/linear need no id to skip.
///
/// Spec 014 F1: on `Ok(Applied)` a `tracker.blocked` event is emitted on the
/// bus (fire-and-forget, same rules as the pipeline seam). `reason` in the
/// payload is the caller's `gate_label`.
///
/// Spec 014 F4: `with_comment: false` suppresses only the explanatory comment
/// (crash-loop cooldown); the label edit and Projects Blocked-column write are
/// unchanged. The harness retries-exhausted caller passes `true`.
#[allow(clippy::too_many_arguments)]
pub async fn apply_blocked_transition(
    store: &Store,
    provider: &str,
    tracker_id: &str,
    tracker_url: Option<&str>,
    feature_name: &str,
    gate_label: &str,
    attempts: u32,
    gate_tail: &str,
    with_comment: bool,
    emit: TrackerEmit<'_>,
) -> anyhow::Result<TransitionResult> {
    let result = blocked_inner(
        store,
        provider,
        tracker_id,
        tracker_url,
        feature_name,
        gate_label,
        attempts,
        gate_tail,
        with_comment,
    )
    .await;
    if matches!(result, Ok(TransitionResult::Applied)) {
        let _ = emit
            .bus
            .send(
                agentum_core::Event::new("tracker.blocked").with_payload(serde_json::json!({
                    "worktree_id": emit.worktree_id,
                    "provider": provider,
                    "tracker_url": tracker_url,
                    "reason": gate_label,
                })),
            );
    }
    result
}

/// The pre-014 blocked body (see [`transition_inner`]), plus the F4
/// `with_comment` thread-through.
#[allow(clippy::too_many_arguments)]
async fn blocked_inner(
    store: &Store,
    provider: &str,
    tracker_id: &str,
    tracker_url: Option<&str>,
    feature_name: &str,
    gate_label: &str,
    attempts: u32,
    gate_tail: &str,
    with_comment: bool,
) -> anyhow::Result<TransitionResult> {
    let _ = (store, tracker_id);
    match provider {
        "github" => {
            let Some(url) = tracker_url.map(str::trim).filter(|u| !u.is_empty()) else {
                return Ok(TransitionResult::Skipped(
                    "feature has no tracker_url; owner/repo unknown".into(),
                ));
            };
            let Some((slug, number)) = github_slug_and_number_from_issue_url(url) else {
                return Ok(TransitionResult::Skipped(format!(
                    "cannot parse a GitHub issue from {url}"
                )));
            };
            let map = GithubStateMap::from_env();
            // Spec 010 F2: binding read only AFTER the parse (the hermeticity
            // discipline — see the pipeline arm above).
            let binding = crate::github_projects::binding_for_slug(&slug);
            Ok(github_mark_blocked_with_board(
                &gh_bin(),
                &slug,
                &number,
                feature_name,
                gate_label,
                attempts,
                gate_tail,
                with_comment,
                &map,
                binding.as_ref(),
            )
            .await)
        }
        // Board and Linear model no "blocked" state (D-A); the D6 label lives
        // only on GitHub. A best-effort no-op, never an error.
        "board" | "linear" => Ok(TransitionResult::Skipped("no blocked state".into())),
        other => Ok(TransitionResult::Skipped(format!(
            "unknown tracker provider {other:?}"
        ))),
    }
}

/// `gh issue create` argv for a non-interactive create, plus one `--label <l>`
/// per non-blank label (spec 003). Returns owned `String`s (not a fixed array)
/// because the label count is dynamic. Pure helper so the shape is unit-tested
/// without spawning a process; labels are argv tokens (never shell-interpolated).
fn gh_create_argv(title: &str, body: &str, labels: &[String]) -> Vec<String> {
    let mut argv = vec![
        "issue".into(),
        "create".into(),
        "--title".into(),
        title.into(),
        "--body".into(),
        body.into(),
    ];
    push_label_args(&mut argv, labels);
    argv
}

/// `gh issue create --repo <slug>` argv (spec 019) + `--label` flags (spec 003).
/// The explicit `--repo` makes `gh` file against `owner/repo` regardless of the
/// cwd's git remote, so this is runnable from any readable dir. Pure helper so the
/// shape is unit-tested without spawning a process. The slug and labels are argv
/// tokens (never interpolated into a shell), so a malformed value fails at `gh`,
/// not via injection.
fn gh_create_argv_with_repo(slug: &str, title: &str, body: &str, labels: &[String]) -> Vec<String> {
    let mut argv = vec![
        "issue".into(),
        "create".into(),
        "--repo".into(),
        slug.into(),
        "--title".into(),
        title.into(),
        "--body".into(),
        body.into(),
    ];
    push_label_args(&mut argv, labels);
    argv
}

/// Append a `--label <l>` pair for each non-blank, trimmed label. Blank labels
/// are skipped so a stray empty string from the UI never becomes a `--label ""`.
fn push_label_args(argv: &mut Vec<String>, labels: &[String]) {
    for l in labels {
        let l = l.trim();
        if !l.is_empty() {
            argv.push("--label".into());
            argv.push(l.to_string());
        }
    }
}

/// A neutral, always-present cwd for an explicit-`--repo` `gh` call: `$HOME`,
/// falling back to the system temp dir. Avoids using a project `workdir` that
/// may not exist locally (the spec 019 bug) and keeps a stray `.git`/`GH_REPO`
/// in some other dir from interfering with the explicit `--repo` target.
///
/// `pub(crate)` so the read-only issue-body fetch (`routes::github`) runs `gh`
/// from the same neutral cwd as issue creation (spec 002).
pub(crate) fn neutral_cwd() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// Parse the issue URL `gh issue create` prints to stdout into a [`FeatureRef`].
/// The number (after `/issues/`) becomes the harness feature id; the full URL is
/// surfaced to the user. `pub(crate)` so the remote (SSH) GitHub path in
/// `routes::board_goals` can reuse it against `gh`'s stdout from `gh_in_dir`
/// (spec 018 S3) — same parser, local or remote.
pub(crate) fn parse_gh_issue_url(stdout: &str) -> anyhow::Result<FeatureRef> {
    let url = stdout
        .lines()
        .map(str::trim)
        .rev()
        .find(|l| l.contains("/issues/"))
        .ok_or_else(|| anyhow::anyhow!("gh issue create produced no issue URL:\n{stdout}"))?;
    let number = url
        .rsplit('/')
        .find(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("could not parse issue number from {url}"))?;
    Ok(FeatureRef {
        provider: "github",
        id: number.to_string(),
        url: Some(url.to_string()),
    })
}

/// GitHub is usable as a task sink when `gh` is on PATH and `workdir` is a
/// GitHub repo the authenticated user can see. `gh repo view` covers
/// install + auth + repo in one cheap call (it fails outside a gh-resolvable
/// repo or when logged out).
async fn github_ready(workdir: &Path) -> bool {
    if which::which("gh").is_err() {
        return false;
    }
    matches!(
        tokio::process::Command::new("gh")
            .args(["repo", "view", "--json", "name"])
            .current_dir(workdir)
            .output()
            .await,
        Ok(o) if o.status.success()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_store::Store;

    async fn fresh_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        // Keep the temp dir alive for the duration of the test process; the
        // store holds an open handle to the file.
        std::mem::forget(dir);
        Store::open(&p).await.unwrap()
    }

    fn ctx<'a>(store: &'a Store, workdir: &'a Path, parent: Option<i64>) -> SinkCtx<'a> {
        SinkCtx {
            store,
            workdir,
            parent_goal_id: parent,
            // Legacy cwd-relative GitHub path; the Chat path sets slug explicitly.
            slug: None,
        }
    }

    #[test]
    fn pick_provider_precedence_github_then_linear_then_board() {
        // External manager = truth when configured; board = agnostic fallback.
        // GitHub wins over Linear when both are present (deterministic).
        assert_eq!(TaskSink::pick_provider(true, true), TaskSink::Github);
        assert_eq!(TaskSink::pick_provider(true, false), TaskSink::Github);
        assert_eq!(TaskSink::pick_provider(false, true), TaskSink::Linear);
        assert_eq!(TaskSink::pick_provider(false, false), TaskSink::Board);
    }

    #[test]
    fn provider_ids_are_stable() {
        assert_eq!(TaskSink::Board.provider(), "board");
        assert_eq!(TaskSink::Github.provider(), "github");
        assert_eq!(TaskSink::Linear.provider(), "linear");
    }

    #[test]
    fn gh_create_argv_is_noninteractive() {
        let argv = gh_create_argv("My title", "My body", &[]);
        assert_eq!(
            argv,
            [
                "issue", "create", "--title", "My title", "--body", "My body"
            ]
        );
    }

    /// Spec 019: the explicit-`--repo` argv carries `--repo <slug>` so `gh`
    /// files against `owner/repo` regardless of cwd. The slug is a single argv
    /// token (never shell-interpolated).
    #[test]
    fn gh_create_argv_with_repo_targets_the_slug() {
        let argv = gh_create_argv_with_repo("owner/repo", "My title", "My body", &[]);
        assert_eq!(
            argv,
            [
                "issue",
                "create",
                "--repo",
                "owner/repo",
                "--title",
                "My title",
                "--body",
                "My body"
            ]
        );
    }

    /// Spec 003: each non-blank label becomes a trailing `--label <l>` pair;
    /// blank labels are dropped (never a `--label ""`).
    #[test]
    fn gh_create_argv_appends_labels() {
        let labels = vec![
            "enhancement".to_string(),
            "  ".to_string(),
            "area/chat".to_string(),
        ];
        let argv = gh_create_argv("T", "B", &labels);
        assert_eq!(
            argv,
            [
                "issue",
                "create",
                "--title",
                "T",
                "--body",
                "B",
                "--label",
                "enhancement",
                "--label",
                "area/chat"
            ]
        );
        // Same tail on the explicit-repo argv.
        let argv = gh_create_argv_with_repo("o/r", "T", "B", &labels);
        assert_eq!(
            &argv[argv.len() - 4..],
            &["--label", "enhancement", "--label", "area/chat"]
        );
    }

    #[test]
    fn parse_gh_issue_url_extracts_number_and_url() {
        // gh prints a banner line or two then the URL last.
        let out = "Creating issue in owner/repo\nhttps://github.com/owner/repo/issues/42\n";
        let r = parse_gh_issue_url(out).unwrap();
        assert_eq!(r.provider, "github");
        assert_eq!(r.id, "42");
        assert_eq!(
            r.url.as_deref(),
            Some("https://github.com/owner/repo/issues/42")
        );
    }

    #[test]
    fn parse_gh_issue_url_errors_without_url() {
        assert!(parse_gh_issue_url("nothing useful here\n").is_err());
    }

    #[test]
    fn github_status_label_covers_all_phases_uniquely() {
        let labels = [
            github_status_label(TrackerPhase::Todo),
            github_status_label(TrackerPhase::InProgress),
            github_status_label(TrackerPhase::InReview),
            github_status_label(TrackerPhase::ReadyToTest),
            github_status_label(TrackerPhase::Done),
        ];
        assert_eq!(
            labels,
            [
                "status/todo",
                "status/in-progress",
                "status/in-review",
                "status/ready-to-test",
                "status/done"
            ]
        );
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), 5, "labels must be pairwise distinct");
    }

    /// `--force` makes the ensure-create idempotent (an existing label is
    /// updated to the canonical color instead of failing the call).
    #[test]
    fn gh_label_ensure_argv_is_idempotent_shape() {
        let argv = gh_label_ensure_argv("status/todo", "owner/repo", "ededed");
        assert_eq!(
            argv,
            [
                "label",
                "create",
                "status/todo",
                "--repo",
                "owner/repo",
                "--color",
                "ededed",
                "--force"
            ]
        );
    }

    #[test]
    fn github_slug_and_number_from_issue_url_parses_and_rejects() {
        // The canonical URL `gh issue create` prints.
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/owner/repo/issues/42"),
            Some(("owner/repo".into(), "42".into()))
        );
        // Tolerated: surrounding whitespace, trailing slash, query, fragment.
        assert_eq!(
            github_slug_and_number_from_issue_url(" https://github.com/o/r/issues/7/ "),
            Some(("o/r".into(), "7".into()))
        );
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/o/r/issues/7?foo=bar"),
            Some(("o/r".into(), "7".into()))
        );
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/o/r/issues/7#issuecomment-1"),
            Some(("o/r".into(), "7".into()))
        );
        // Rejected: PR links, non-github hosts, non-numeric tails, junk.
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/o/r/pull/42"),
            None
        );
        assert_eq!(
            github_slug_and_number_from_issue_url("https://gitlab.com/o/r/issues/42"),
            None
        );
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/o/r/issues/abc"),
            None
        );
        assert_eq!(
            github_slug_and_number_from_issue_url("https://github.com/o/r/issues/42/comments"),
            None
        );
        assert_eq!(github_slug_and_number_from_issue_url("not a url"), None);
        assert_eq!(github_slug_and_number_from_issue_url(""), None);
    }

    /// Spec 004 (C4 invariant at argv level) + spec 008 D6 + spec 012 F3: one
    /// `gh issue edit` adds the target label and removes exactly the OTHER FIVE
    /// canonical labels — the four other pipeline phases AND the fixed
    /// `status/blocked` — so a pipeline flip also clears a lingering blocked
    /// label (the board can't lie in either direction). The target is never
    /// removed, and no non-canonical name (e.g. this repo's own `status/qa*`
    /// human-QA labels) appears.
    ///
    /// The `expected` literals carry the D6 `--remove-label status/blocked` tail
    /// and the F3 `status/in-review` label; the pipeline order/tokens are
    /// otherwise the spec 004/005 shape. Do not regenerate them from the code
    /// under test.
    #[test]
    fn gh_set_status_label_argv_adds_one_removes_the_four_pipeline_and_blocked() {
        let all_phases = [
            TrackerPhase::Todo,
            TrackerPhase::InProgress,
            TrackerPhase::InReview,
            TrackerPhase::ReadyToTest,
            TrackerPhase::Done,
        ];
        let map = GithubStateMap::default();
        for phase in all_phases {
            let target = github_status_label(phase);
            let argv = gh_set_status_label_argv("42", "owner/repo", phase, &map);
            let expected: Vec<&str> = match phase {
                TrackerPhase::Todo => vec![
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    "status/todo",
                    "--remove-label",
                    "status/in-progress",
                    "--remove-label",
                    "status/in-review",
                    "--remove-label",
                    "status/ready-to-test",
                    "--remove-label",
                    "status/done",
                    "--remove-label",
                    "status/blocked",
                ],
                TrackerPhase::InProgress => vec![
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    "status/in-progress",
                    "--remove-label",
                    "status/todo",
                    "--remove-label",
                    "status/in-review",
                    "--remove-label",
                    "status/ready-to-test",
                    "--remove-label",
                    "status/done",
                    "--remove-label",
                    "status/blocked",
                ],
                TrackerPhase::InReview => vec![
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    "status/in-review",
                    "--remove-label",
                    "status/todo",
                    "--remove-label",
                    "status/in-progress",
                    "--remove-label",
                    "status/ready-to-test",
                    "--remove-label",
                    "status/done",
                    "--remove-label",
                    "status/blocked",
                ],
                TrackerPhase::ReadyToTest => vec![
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    "status/ready-to-test",
                    "--remove-label",
                    "status/todo",
                    "--remove-label",
                    "status/in-progress",
                    "--remove-label",
                    "status/in-review",
                    "--remove-label",
                    "status/done",
                    "--remove-label",
                    "status/blocked",
                ],
                TrackerPhase::Done => vec![
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    "status/done",
                    "--remove-label",
                    "status/todo",
                    "--remove-label",
                    "status/in-progress",
                    "--remove-label",
                    "status/in-review",
                    "--remove-label",
                    "status/ready-to-test",
                    "--remove-label",
                    "status/blocked",
                ],
            };
            assert_eq!(argv, expected, "default-map argv drifted for {phase:?}");
            // Head: one edit targeting the issue + repo, adding exactly the target.
            assert_eq!(
                &argv[..7],
                &[
                    "issue",
                    "edit",
                    "42",
                    "--repo",
                    "owner/repo",
                    "--add-label",
                    target
                ]
            );
            // Tail: a `--remove-label <l>` pair for each of the other four.
            let removed: Vec<&str> = argv[7..]
                .chunks(2)
                .map(|pair| {
                    assert_eq!(pair[0], "--remove-label");
                    pair[1]
                })
                .collect();
            assert_eq!(removed.len(), 5, "four pipeline + blocked removed");
            assert!(
                !removed.contains(&target),
                "the target label must never be removed"
            );
            for r in &removed {
                assert!(
                    GITHUB_STATUS_LABELS.iter().any(|(_, name, _)| name == r)
                        || *r == GITHUB_BLOCKED_LABEL.0,
                    "non-canonical label {r} in the remove set (C4 violation)"
                );
            }
            for (p, name, _) in GITHUB_STATUS_LABELS.iter() {
                if *p != phase {
                    assert!(removed.contains(name), "{name} missing from remove set");
                }
            }
            assert!(
                removed.contains(&GITHUB_BLOCKED_LABEL.0),
                "every pipeline flip must also clear status/blocked (D6)"
            );
        }
    }

    /// Spec 005 F5: the default map IS the canonical `GITHUB_STATUS_LABELS`
    /// name set, and `label_for` agrees with the const-table accessor.
    #[test]
    fn github_state_map_defaults_are_canonical() {
        let m = GithubStateMap::default();
        assert_eq!(m.todo, "status/todo");
        assert_eq!(m.in_progress, "status/in-progress");
        assert_eq!(m.in_review, "status/in-review");
        assert_eq!(m.ready_to_test, "status/ready-to-test");
        assert_eq!(m.done, "status/done");
        for (p, name, _) in GITHUB_STATUS_LABELS.iter() {
            assert_eq!(m.label_for(*p), *name);
            assert_eq!(github_status_label(*p), *name);
        }
    }

    /// Spec 005 F5 layering, via the pure `apply_layers` injection — NO env
    /// mutation, no config file: defaults → file → env, with blank/whitespace
    /// values at any layer keeping the lower layer, and values trimmed.
    #[test]
    fn github_state_map_precedence_file_then_env() {
        let no_env = |_: &str| None;
        // No file, no env → defaults.
        assert_eq!(
            GithubStateMap::apply_layers(None, no_env),
            GithubStateMap::default()
        );
        // File overrides defaults; a partial/blank file keeps defaults.
        let file = StoredGithubStateMap {
            todo: Some("triage".into()),
            in_progress: Some(" wip ".into()), // trimmed
            in_review: None,
            ready_to_test: None,
            done: Some("   ".into()), // blank keeps the lower layer
        };
        let m = GithubStateMap::apply_layers(Some(file), no_env);
        assert_eq!(m.todo, "triage");
        assert_eq!(m.in_progress, "wip");
        assert_eq!(m.ready_to_test, "status/ready-to-test");
        assert_eq!(m.done, "status/done");
        // Env wins over the file; a blank env value keeps the file layer.
        let file = StoredGithubStateMap {
            todo: Some("triage".into()),
            in_progress: Some("wip".into()),
            ..Default::default()
        };
        let env = |k: &str| match k {
            "AGENTUM_GITHUB_STATUS_TODO" => Some(" backlog ".to_string()),
            "AGENTUM_GITHUB_STATUS_IN_PROGRESS" => Some("".to_string()),
            "AGENTUM_GITHUB_STATUS_DONE" => Some("shipped".to_string()),
            _ => None,
        };
        let m = GithubStateMap::apply_layers(Some(file), env);
        assert_eq!(m.todo, "backlog"); // env over file, trimmed
        assert_eq!(m.in_progress, "wip"); // blank env keeps the file value
        assert_eq!(m.ready_to_test, "status/ready-to-test"); // default survives
        assert_eq!(m.done, "shipped"); // env over default
    }

    /// Spec 005 F5 (the mid-flight/foreign-label pin at argv level): a fully
    /// renamed map produces an argv of ONLY the configured names — the target
    /// custom name is added, the other three custom names are removed, and no
    /// canonical default appears anywhere.
    #[test]
    fn gh_set_status_label_argv_uses_configured_names() {
        let map = GithubStateMap {
            todo: "triage".into(),
            in_progress: "wip".into(),
            in_review: "reviewing".into(),
            ready_to_test: "qa-ready".into(),
            done: "shipped".into(),
        };
        let argv = gh_set_status_label_argv("7", "o/r", TrackerPhase::InProgress, &map);
        assert_eq!(
            argv,
            vec![
                "issue",
                "edit",
                "7",
                "--repo",
                "o/r",
                "--add-label",
                "wip",
                "--remove-label",
                "triage",
                "--remove-label",
                "reviewing",
                "--remove-label",
                "qa-ready",
                "--remove-label",
                "shipped",
                // Spec 008 D6: the fixed blocked label is cleared alongside the
                // configured pipeline names.
                "--remove-label",
                "status/blocked",
            ]
        );
        for (_, canonical, _) in GITHUB_STATUS_LABELS.iter() {
            assert!(
                !argv.contains(canonical),
                "canonical default {canonical} leaked into a custom-map argv"
            );
        }
    }

    /// Spec 005 F5: the remove-set filters by NAME. Two phases mapped to one
    /// name: when that name is the target it is added and NEVER removed; when
    /// it is not the target it is removed exactly once (deduped).
    #[test]
    fn gh_set_status_label_argv_never_removes_the_target_on_name_collision() {
        let map = GithubStateMap {
            todo: "status/todo".into(),
            in_progress: "active".into(),
            in_review: "status/in-review".into(),
            ready_to_test: "active".into(),
            done: "status/done".into(),
        };
        for phase in [TrackerPhase::InProgress, TrackerPhase::ReadyToTest] {
            let argv = gh_set_status_label_argv("42", "o/r", phase, &map);
            assert_eq!(&argv[5..7], &["--add-label", "active"]);
            let removed: Vec<&str> = argv[7..]
                .chunks(2)
                .map(|pair| {
                    assert_eq!(pair[0], "--remove-label");
                    pair[1]
                })
                .collect();
            assert_eq!(
                removed,
                // …plus the F3 in-review label and the D6 blocked label (spec 008).
                [
                    "status/todo",
                    "status/in-review",
                    "status/done",
                    "status/blocked"
                ],
                "the shared target must be absent from its own remove list"
            );
        }
        // Shared name NOT the target → removed once, not twice (+ blocked, D6).
        let argv = gh_set_status_label_argv("42", "o/r", TrackerPhase::Done, &map);
        let removed: Vec<&str> = argv[7..].chunks(2).map(|pair| pair[1]).collect();
        assert_eq!(
            removed,
            [
                "status/todo",
                "active",
                "status/in-review",
                "status/blocked"
            ]
        );
    }

    /// Spec 005 F5 (§6 item 6), extended spec 012 F3: a fake `gh` logs the full
    /// custom-map transition — 5 ensure-creates carrying the CUSTOM names with the
    /// canonical PHASE colors, then one edit that adds/removes only configured
    /// names. Explicit `program` + explicit `map` → no env mutation, no lock.
    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_with_custom_map_flips_configured_names() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );
        let map = GithubStateMap {
            todo: "triage".into(),
            in_progress: "wip".into(),
            in_review: "reviewing".into(),
            ready_to_test: "qa-ready".into(),
            done: "shipped".into(),
        };

        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::ReadyToTest,
            &map,
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 6, "5 ensure-creates + 1 edit, got: {calls}");
        assert_eq!(
            lines[..5],
            [
                "label create triage --repo owner/repo --color ededed --force",
                "label create wip --repo owner/repo --color 1d76db --force",
                "label create reviewing --repo owner/repo --color 5319e7 --force",
                "label create qa-ready --repo owner/repo --color fbca04 --force",
                "label create shipped --repo owner/repo --color 0e8a16 --force",
            ]
        );
        assert_eq!(
            lines[5],
            "issue edit 42 --repo owner/repo --add-label qa-ready \
             --remove-label triage --remove-label wip --remove-label reviewing \
             --remove-label shipped --remove-label status/blocked"
        );
        for (_, canonical, _) in GITHUB_STATUS_LABELS.iter() {
            assert!(
                !calls.contains(canonical),
                "canonical default {canonical} leaked into a custom-map transition"
            );
        }
    }

    /// Spec 005 F5: the ensure-loop dedupes by name — a shared name is
    /// ensure-created once, with the FIRST phase's canonical color winning.
    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_ensures_duplicate_names_once() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );
        let map = GithubStateMap {
            todo: "status/todo".into(),
            in_progress: "active".into(),
            in_review: "status/in-review".into(),
            ready_to_test: "active".into(),
            done: "status/done".into(),
        };

        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::Done,
            &map,
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 5, "4 deduped ensure-creates + 1 edit: {calls}");
        assert_eq!(
            lines[..4],
            [
                "label create status/todo --repo owner/repo --color ededed --force",
                // InProgress comes first in canonical order, so its color wins.
                "label create active --repo owner/repo --color 1d76db --force",
                "label create status/in-review --repo owner/repo --color 5319e7 --force",
                "label create status/done --repo owner/repo --color 0e8a16 --force",
            ]
        );
    }

    /// Spec 005 F4 + spec 012 F3: the wire-format phase parser accepts exactly
    /// the five pipeline phases and rejects everything else (case-sensitive, no
    /// aliases).
    #[test]
    fn parse_tracker_phase_accepts_the_five_and_rejects_junk() {
        assert_eq!(parse_tracker_phase("todo"), Some(TrackerPhase::Todo));
        assert_eq!(
            parse_tracker_phase("in_progress"),
            Some(TrackerPhase::InProgress)
        );
        assert_eq!(
            parse_tracker_phase("in_review"),
            Some(TrackerPhase::InReview)
        );
        assert_eq!(
            parse_tracker_phase("ready_to_test"),
            Some(TrackerPhase::ReadyToTest)
        );
        assert_eq!(parse_tracker_phase("done"), Some(TrackerPhase::Done));
        for junk in ["", "Todo", "DONE", "in-review", "ready to test", "qa"] {
            assert_eq!(parse_tracker_phase(junk), None, "{junk:?} must be rejected");
        }
    }

    #[test]
    fn board_status_mapping_covers_all_phases() {
        assert_eq!(board_status_for(TrackerPhase::Todo), "todo");
        assert_eq!(board_status_for(TrackerPhase::InProgress), "doing");
        // InReview folds onto the internal board's `review` column alongside
        // ReadyToTest (no distinct in-review board column).
        assert_eq!(board_status_for(TrackerPhase::InReview), "review");
        assert_eq!(board_status_for(TrackerPhase::ReadyToTest), "review");
        assert_eq!(board_status_for(TrackerPhase::Done), "done");
    }

    /// A throwaway bus for the seam's required `TrackerEmit` (spec 014 F1) —
    /// tests that don't assert emission just need a live sender to pass.
    fn test_bus() -> tokio::sync::broadcast::Sender<agentum_core::Event> {
        tokio::sync::broadcast::channel(8).0
    }

    #[tokio::test]
    async fn board_transition_moves_card_status() {
        let store = fresh_store().await;
        let here = std::env::temp_dir();
        let r = TaskSink::Board
            .create_feature(
                &ctx(&store, &here, None),
                &NewFeature {
                    title: "Add OAuth login".into(),
                    body: None,
                    labels: vec![],
                },
            )
            .await
            .unwrap();

        let bus = test_bus();
        let res = apply_tracker_transition(
            &store,
            "board",
            &r.id,
            None,
            TrackerPhase::InProgress,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(res, TransitionResult::Applied);
        let card = store
            .list_board_items()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.key == r.id)
            .unwrap();
        assert_eq!(card.status, "doing");
    }

    #[tokio::test]
    async fn board_transition_unknown_key_is_skipped() {
        let store = fresh_store().await;
        let bus = test_bus();
        let res = apply_tracker_transition(
            &store,
            "board",
            "AG-9999",
            None,
            TrackerPhase::Done,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
    }

    /// Spec 004: replaces `github_transition_is_a_logged_noop`. The GitHub arm
    /// is real now, but stays best-effort: no URL (owner/repo unknown) or an
    /// unparseable URL → `Ok(Skipped)`, never `Err` (AC 5).
    #[tokio::test]
    async fn github_transition_without_url_is_skipped() {
        let store = fresh_store().await;
        let bus = test_bus();
        let res = apply_tracker_transition(
            &store,
            "github",
            "42",
            None,
            TrackerPhase::Done,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
        // Blank and unparseable (a /pull/ link) URLs are skips too.
        let res = apply_tracker_transition(
            &store,
            "github",
            "42",
            Some("  "),
            TrackerPhase::Done,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
        let res = apply_tracker_transition(
            &store,
            "github",
            "42",
            Some("https://github.com/o/r/pull/42"),
            TrackerPhase::Done,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
    }

    // ---- Spec 014 F1: bus emission at the seam --------------------------------

    /// AC 1: an `Applied` transition emits exactly one `tracker.phase_changed`
    /// with the full payload. Driven through the hermetic board arm — the
    /// emission choke point is upstream of provider dispatch, so this proves it
    /// for all providers (the gh transport is covered by the fake-gh tests).
    #[tokio::test]
    async fn applied_transition_emits_phase_changed_on_bus() {
        let store = fresh_store().await;
        let here = std::env::temp_dir();
        let r = TaskSink::Board
            .create_feature(
                &ctx(&store, &here, None),
                &NewFeature {
                    title: "Emit on applied".into(),
                    body: None,
                    labels: vec![],
                },
            )
            .await
            .unwrap();

        let (bus, mut rx) = tokio::sync::broadcast::channel(8);
        let res = apply_tracker_transition(
            &store,
            "board",
            &r.id,
            None,
            TrackerPhase::InProgress,
            TrackerEmit {
                bus: &bus,
                worktree_id: Some("repo-1::/tmp/wt"),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, TransitionResult::Applied);

        let ev = rx.try_recv().expect("Applied emits one event");
        assert_eq!(ev.kind, "tracker.phase_changed");
        assert_eq!(ev.payload["worktree_id"], "repo-1::/tmp/wt");
        assert_eq!(ev.payload["provider"], "board");
        assert_eq!(ev.payload["phase"], "in_progress");
        assert!(ev.payload["tracker_url"].is_null(), "board has no URL");
        assert!(
            rx.try_recv().is_err(),
            "exactly ONE event per applied transition"
        );
    }

    /// AC 2: a skipped transition emits NOTHING — the bus never lies. Covers
    /// the board unknown-key skip and the github hermetic no-url/unparseable
    /// skips (the blocked seam shares the same wrapper shape).
    #[tokio::test]
    async fn skipped_transition_emits_nothing() {
        let store = fresh_store().await;
        let (bus, mut rx) = tokio::sync::broadcast::channel(8);
        let emit = || TrackerEmit {
            bus: &bus,
            worktree_id: None,
        };

        let res =
            apply_tracker_transition(&store, "board", "AG-9999", None, TrackerPhase::Done, emit())
                .await
                .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));

        let res =
            apply_tracker_transition(&store, "github", "42", None, TrackerPhase::Done, emit())
                .await
                .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));

        let res = apply_blocked_transition(
            &store,
            "github",
            "42",
            None,
            "feat",
            "gate",
            1,
            "boom",
            true,
            emit(),
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));

        assert!(rx.try_recv().is_err(), "Skipped must emit nothing");
    }

    /// Write an executable fake `gh` into `dir` and return its path. Passed to
    /// `github_transition_with` as the explicit `program` — no env mutation,
    /// no lock needed (the AGENTUM_GH_BIN env var is only the production knob).
    #[cfg(unix)]
    fn write_fake_gh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("gh-fake");
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_applies_with_fake_gh() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );

        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::InProgress,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        // 5 ensure-creates then the single issue edit, in that order.
        assert_eq!(lines.len(), 6, "expected 6 gh invocations, got: {calls}");
        for line in &lines[..5] {
            assert!(
                line.starts_with("label create status/"),
                "expected an ensure-create, got: {line}"
            );
            assert!(line.ends_with("--force"));
        }
        assert!(
            lines[5].starts_with("issue edit 42 --repo owner/repo --add-label status/in-progress"),
            "last call must be the issue edit, got: {}",
            lines[5]
        );
    }

    /// Spec 012 F3 (the first-failing test): the new `InReview` phase writes the
    /// `status/in-review` label and removes exactly the OTHER FOUR pipeline names
    /// plus `status/blocked` — the five-label mutual exclusion. Fake-`gh`, no env.
    #[cfg(unix)]
    #[tokio::test]
    async fn inreview_writes_in_review_label_and_removes_other_four() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );

        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::InReview,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 6, "5 ensure-creates + 1 edit, got: {calls}");
        // The single edit adds in-review and removes the other four + blocked.
        assert_eq!(
            lines[5],
            "issue edit 42 --repo owner/repo --add-label status/in-review \
             --remove-label status/todo --remove-label status/in-progress \
             --remove-label status/ready-to-test --remove-label status/done \
             --remove-label status/blocked"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_maps_gh_failure_to_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_fake_gh(
            dir.path(),
            "#!/bin/sh\necho 'boom: label not found' >&2\nexit 1\n",
        );

        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::Done,
            &GithubStateMap::default(),
        )
        .await;
        // Ensure-create failures are non-fatal; the failed edit surfaces its
        // stderr as the Skipped reason — and it is never an Err (AC 5).
        match res {
            TransitionResult::Skipped(reason) => {
                assert!(reason.contains("boom: label not found"), "got: {reason}")
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    // ---- Spec 008 D6: the `status/blocked` escalation ------------------------

    /// The blocked argv adds `status/blocked` and removes the FIVE pipeline
    /// names (the mirror of `gh_set_status_label_argv` with a fixed target;
    /// spec 012 F3 added `status/in-review`).
    #[test]
    fn gh_set_blocked_label_argv_adds_blocked_removes_five_pipeline() {
        let map = GithubStateMap::default();
        let argv = gh_set_blocked_label_argv("42", "owner/repo", &map);
        assert_eq!(
            argv,
            vec![
                "issue",
                "edit",
                "42",
                "--repo",
                "owner/repo",
                "--add-label",
                "status/blocked",
                "--remove-label",
                "status/todo",
                "--remove-label",
                "status/in-progress",
                "--remove-label",
                "status/in-review",
                "--remove-label",
                "status/ready-to-test",
                "--remove-label",
                "status/done",
            ]
        );
    }

    /// The comment body is a single argv token — a multi-line body (code fence)
    /// is never split on newlines, so `gh` receives it verbatim.
    #[test]
    fn gh_issue_comment_argv_body_is_a_single_token() {
        let body = "line one\nline two ```code```";
        let argv = gh_issue_comment_argv("42", "owner/repo", body);
        assert_eq!(
            argv,
            [
                "issue",
                "comment",
                "42",
                "--repo",
                "owner/repo",
                "--body",
                body
            ]
        );
        assert_eq!(argv[6], body, "the whole body is one token");
    }

    /// The AC-4 comment carries the feature name, the retry count, the gate
    /// label, and the gate-output tail, in a GitHub-collapsible fenced block.
    #[test]
    fn blocked_comment_body_carries_attempts_and_gate_tail() {
        let body = blocked_comment_body(
            "Login screen",
            "unit-test gate (verify.sh)",
            3,
            "assertion failed: foo != bar",
        );
        assert!(body.contains("Login screen"), "names the feature: {body}");
        assert!(
            body.contains("unit-test gate (verify.sh)"),
            "names the gate"
        );
        assert!(
            body.contains("3 attempt"),
            "carries the retry count: {body}"
        );
        assert!(
            body.contains("assertion failed: foo != bar"),
            "carries the gate tail: {body}"
        );
        assert!(
            body.contains("<details>"),
            "the tail is collapsible: {body}"
        );
        assert!(body.contains("```"), "the tail is fenced: {body}");
    }

    /// A fake `gh` proves the three-call escalation shape: ensure `status/blocked`
    /// → one edit (add blocked + remove the four pipeline names) → one comment.
    /// Newline-safe: the multi-line comment body is dumped to a file so it never
    /// smears the single-line call log.
    #[cfg(unix)]
    #[tokio::test]
    async fn github_mark_blocked_with_fake_gh() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let body_file = dir.path().join("comment-body.txt");
        let script = write_fake_gh(
            dir.path(),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"comment\" ]; then\n  \
                 echo \"issue comment\" >> \"{log}\"\n  printf '%s' \"$7\" > \"{body}\"\nelse\n  \
                 echo \"$@\" >> \"{log}\"\nfi\nexit 0\n",
                log = log.display(),
                body = body_file.display(),
            ),
        );

        let res = github_mark_blocked_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            "Login screen",
            "unit-test gate (verify.sh)",
            3,
            "assertion failed: foo != bar",
            /* with_comment */ true,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "ensure-create + edit + comment, got: {calls}"
        );
        assert_eq!(
            lines[0],
            "label create status/blocked --repo owner/repo --color b60205 --force"
        );
        assert_eq!(
            lines[1],
            "issue edit 42 --repo owner/repo --add-label status/blocked \
             --remove-label status/todo --remove-label status/in-progress \
             --remove-label status/in-review --remove-label status/ready-to-test \
             --remove-label status/done"
        );
        assert_eq!(lines[2], "issue comment");

        let body = std::fs::read_to_string(&body_file).unwrap();
        assert!(
            body.contains("Login screen"),
            "comment names the feature: {body}"
        );
        assert!(
            body.contains("3 attempt"),
            "comment carries the retry count: {body}"
        );
        assert!(
            body.contains("assertion failed: foo != bar"),
            "comment carries the gate tail: {body}"
        );
    }

    // ---- Spec 014 F4: comment suppression + clear flow ------------------------

    /// AC 10 crash-loop guard at the seam: `with_comment=false` runs the
    /// idempotent label edit but ZERO `issue comment`; a true+false pair (what
    /// the attention ledger decides for two crashes inside the cooldown)
    /// leaves TWO label edits and exactly ONE comment in the log.
    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_with_comment_false_suppresses_only_the_comment() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"comment\" ]; then\n  \
                 echo \"issue comment\" >> \"{log}\"\nelse\n  \
                 echo \"$@\" >> \"{log}\"\nfi\nexit 0\n",
                log = log.display(),
            ),
        );

        // Crash #1: a fresh episode — label + comment.
        let res = github_mark_blocked_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            "session-a",
            "session crash",
            1,
            "panic: boom",
            /* with_comment */ true,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);
        // Crash #2 inside the cooldown: the ledger says LabelOnly — the label
        // re-applies (idempotent), the comment is suppressed.
        let res = github_mark_blocked_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            "session-a",
            "session crash",
            1,
            "panic: boom again",
            /* with_comment */ false,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 5, "2×(ensure + edit) + ONE comment: {calls}");
        let edits = lines
            .iter()
            .filter(|l| l.starts_with("issue edit 42 --repo owner/repo --add-label status/blocked"))
            .count();
        assert_eq!(edits, 2, "the label edit runs both times: {calls}");
        let comments = lines.iter().filter(|l| **l == "issue comment").count();
        assert_eq!(comments, 1, "exactly ONE comment across the loop: {calls}");
    }

    /// AC 10 clear flow: after a blocked write, the recovery re-apply (any
    /// pipeline edit) removes `status/blocked` in the SAME `issue edit` — the
    /// board can't stay stale-red.
    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_then_pipeline_transition_removes_blocked_label() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );

        let res = github_mark_blocked_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            "session-a",
            "awaiting input",
            1,
            "stuck",
            true,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);
        // Recovery: the attention worker re-applies the persisted phase
        // verbatim through the pipeline seam.
        let res = github_transition_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::InProgress,
            &GithubStateMap::default(),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let pipeline_edit = calls
            .lines()
            .find(|l| l.contains("--add-label status/in-progress"))
            .expect("the recovery pipeline edit ran");
        assert!(
            pipeline_edit.contains("--remove-label status/blocked"),
            "the re-apply drops the blocked label: {pipeline_edit}"
        );
    }

    /// Never-halt (AC 8): a failing `gh` degrades the blocked write to
    /// `Skipped` — never an `Err`/panic the worker loop would die on.
    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_write_gh_failure_is_skipped_never_halts() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_fake_gh(
            dir.path(),
            "#!/bin/sh\necho 'boom: gh failed' >&2\nexit 1\n",
        );
        let res = github_mark_blocked_with(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            "session-a",
            "session crash",
            1,
            "panic",
            true,
            &GithubStateMap::default(),
        )
        .await;
        match res {
            TransitionResult::Skipped(reason) => {
                assert!(reason.contains("boom: gh failed"), "got: {reason}")
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    /// D-A: board and Linear have no blocked column, so `apply_blocked_transition`
    /// skips them (never `Err`) — the D6 label is GitHub-only. A github.com URL is
    /// passed to prove they skip on provider, not on a missing URL.
    #[tokio::test]
    async fn apply_blocked_transition_board_and_linear_are_skipped() {
        let store = fresh_store().await;
        let bus = test_bus();
        for provider in ["board", "linear"] {
            let res = apply_blocked_transition(
                &store,
                provider,
                "AG-1",
                Some("https://github.com/o/r/issues/1"),
                "feat",
                "unit-test gate",
                2,
                "boom",
                true,
                TrackerEmit {
                    bus: &bus,
                    worktree_id: None,
                },
            )
            .await
            .unwrap();
            match res {
                TransitionResult::Skipped(why) => {
                    assert!(why.contains("no blocked state"), "got: {why}")
                }
                other => panic!("expected Skipped for {provider}, got {other:?}"),
            }
        }
    }

    /// The GitHub arm stays best-effort: a missing/unparseable URL is a
    /// `Skipped`, never an `Err`, and touches no `gh` (hermetic).
    #[tokio::test]
    async fn apply_blocked_transition_github_without_url_is_skipped() {
        let store = fresh_store().await;
        let bus = test_bus();
        let res = apply_blocked_transition(
            &store,
            "github",
            "42",
            None,
            "feat",
            "gate",
            1,
            "boom",
            true,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
        let res = apply_blocked_transition(
            &store,
            "github",
            "42",
            Some("https://github.com/o/r/pull/9"),
            "feat",
            "gate",
            1,
            "boom",
            true,
            TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
    }

    // ---- Spec 010 F2: the additive Projects-board arm -------------------------
    //
    // The binding rides in EXPLICITLY (like `program`/`map`) — never via the
    // config file or env. github_projects' ID_CACHE is process-global, so each
    // test uses its own slug (the established global-state pattern).

    /// A five-phase binding with self-describing option ids for the seam tests.
    #[cfg(unix)]
    fn seam_board_binding() -> crate::github_projects::BoardBinding {
        crate::github_projects::BoardBinding {
            project_id: "PVT_seam".into(),
            status_field_id: "PVTSSF_seam".into(),
            status_mapping: crate::github_projects::StatusMapping {
                todo: "opt-todo".into(),
                in_progress: "opt-inprogress".into(),
                ready_to_test: "opt-rtt".into(),
                done: "opt-done".into(),
                blocked: "opt-blocked".into(),
            },
            done_closes_issue: false,
            project_title: None,
            project_owner: None,
            project_owner_type: None,
            project_number: None,
            option_names: None,
        }
    }

    /// AC 8 at the seam: with NO binding, `github_transition_with_board` IS
    /// today's label path byte-for-byte — the exact 5-invocation log
    /// `github_transition_applies_with_fake_gh` pins, and no GraphQL at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_with_board_unbound_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        );

        let res = github_transition_with_board(
            script.to_str().unwrap(),
            "owner/repo",
            "42",
            TrackerPhase::InProgress,
            &GithubStateMap::default(),
            None,
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 6, "expected 6 gh invocations, got: {calls}");
        for line in &lines[..5] {
            assert!(
                line.starts_with("label create status/"),
                "expected an ensure-create, got: {line}"
            );
            assert!(line.ends_with("--force"));
        }
        assert!(
            lines[5].starts_with("issue edit 42 --repo owner/repo --add-label status/in-progress"),
            "last call must be the issue edit, got: {}",
            lines[5]
        );
        assert!(
            !calls.contains("api graphql"),
            "unbound must never touch GraphQL: {calls}"
        );
    }

    /// THE AC-7 pin: the label edit succeeds but every GraphQL call fails →
    /// the transition comes back `Skipped("status label applied; Projects
    /// board write failed: …")` — a `TransitionResult` the github arm wraps in
    /// `Ok`, so the failure is loud through existing plumbing yet never an
    /// `Err`. The classified scope message keeps its remedy mid-run.
    #[cfg(unix)]
    #[tokio::test]
    async fn github_transition_with_board_board_failure_is_skipped_note_still_ok() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = write_fake_gh(
            dir.path(),
            &format!(
                "#!/bin/sh\necho \"$@\" >> \"{log}\"\n\
                 if [ \"$1\" = \"api\" ]; then\n  \
                 echo 'your token has not been granted the required scopes [read:project]' >&2\n  \
                 exit 1\nfi\nexit 0\n",
                log = log.display()
            ),
        );

        let binding = seam_board_binding();
        let res = github_transition_with_board(
            script.to_str().unwrap(),
            "acme/seam-fail",
            "42",
            TrackerPhase::InProgress,
            &GithubStateMap::default(),
            Some(&binding),
        )
        .await;
        match res {
            TransitionResult::Skipped(reason) => {
                assert!(
                    reason.starts_with("status label applied; Projects board write failed:"),
                    "got: {reason}"
                );
                assert!(
                    reason.contains("gh auth refresh -s project"),
                    "the scope remedy rides into the run log: {reason}"
                );
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        // The whole label path ran (and succeeded) before the board write.
        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|l| l.starts_with("label create status/"))
                .count(),
            5,
            "label ensures untouched: {calls}"
        );
        assert!(calls.contains("issue edit 42"), "label edit ran: {calls}");
    }

    /// Spec 012 F4 (AC 11): the Done transition the poller fires on merge, on a
    /// BOUND repo with `done_closes_issue` ON, drives the full 010 path — the
    /// `status/done` label, the Done-mapped Project OPTION, then the probe-then-
    /// close that closes the still-open issue. Explicit binding (knob ON), no env.
    #[cfg(unix)]
    #[tokio::test]
    async fn done_transition_closes_issue_when_knob_on_with_fake_gh() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let node_body =
            serde_json::json!({"data": {"repository": {"issue": {"id": "I_d"}}}}).to_string();
        let add_body =
            serde_json::json!({"data": {"addProjectV2ItemById": {"item": {"id": "PVTI_d"}}}})
                .to_string();
        let update_body = serde_json::json!(
            {"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": "PVTI_d"}}}}
        )
        .to_string();
        // The issue is OPEN, so knob-ON Done must probe then close it.
        let script = write_fake_gh(
            dir.path(),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"issue\" ] && [ \"$2\" = \"view\" ]; then\n  \
                 echo \"$@\" >> \"{log}\"\n  printf 'OPEN\\n'\n  exit 0\nfi\n\
                 echo \"$@\" >> \"{log}\"\n\
                 case \"$*\" in\n  \
                 *updateProjectV2ItemFieldValue*) printf '%s\\n' '{update_body}' ;;\n  \
                 *addProjectV2ItemById*) printf '%s\\n' '{add_body}' ;;\n  \
                 *repository*) printf '%s\\n' '{node_body}' ;;\n\
                 esac\nexit 0\n",
                log = log.display(),
            ),
        );

        let mut binding = seam_board_binding();
        binding.done_closes_issue = true;
        let res = github_transition_with_board(
            script.to_str().unwrap(),
            "acme/done-closes",
            "42",
            TrackerPhase::Done,
            &GithubStateMap::default(),
            Some(&binding),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        // The Done-mapped OPTION ID rode the Project write.
        assert!(
            calls
                .lines()
                .any(|l| l.contains("updateProjectV2ItemFieldValue")
                    && l.contains("-f option=opt-done")),
            "the Done option rides the write: {calls}"
        );
        // Knob ON + OPEN issue → the probe then the close.
        assert!(
            calls.contains("issue view 42 --repo acme/done-closes --json state --jq .state"),
            "state probe ran: {calls}"
        );
        assert!(
            calls.contains("issue close 42 --repo acme/done-closes"),
            "Done closed the issue (010 done_closes_issue): {calls}"
        );
        // The `status/done` label edit ran too.
        assert!(
            calls.contains("issue edit 42 --repo acme/done-closes --add-label status/done"),
            "the status/done label edit ran: {calls}"
        );
    }

    /// AC 5: the blocked escalation on a BOUND repo adds the card move to the
    /// Blocked-mapped OPTION ID after today's label+comment path — and never
    /// probes/closes/reopens (knob ON notwithstanding: no close/reopen on
    /// Blocked).
    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_arm_moves_card_to_blocked_option_with_fake_gh() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let body_file = dir.path().join("comment-body.txt");
        let node_body =
            serde_json::json!({"data": {"repository": {"issue": {"id": "I_b"}}}}).to_string();
        let add_body =
            serde_json::json!({"data": {"addProjectV2ItemById": {"item": {"id": "PVTI_b"}}}})
                .to_string();
        let update_body = serde_json::json!(
            {"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": "PVTI_b"}}}}
        )
        .to_string();
        let script = write_fake_gh(
            dir.path(),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"issue\" ] && [ \"$2\" = \"comment\" ]; then\n  \
                 echo \"issue comment\" >> \"{log}\"\n  printf '%s' \"$7\" > \"{body}\"\n  exit 0\nfi\n\
                 echo \"$@\" >> \"{log}\"\n\
                 case \"$*\" in\n  \
                 *updateProjectV2ItemFieldValue*) printf '%s\\n' '{update_body}' ;;\n  \
                 *addProjectV2ItemById*) printf '%s\\n' '{add_body}' ;;\n  \
                 *repository*) printf '%s\\n' '{node_body}' ;;\n\
                 esac\nexit 0\n",
                log = log.display(),
                body = body_file.display(),
            ),
        );

        let mut binding = seam_board_binding();
        binding.done_closes_issue = true; // knob ON must still not act on Blocked
        let res = github_mark_blocked_with_board(
            script.to_str().unwrap(),
            "acme/seam-blocked",
            "42",
            "Login screen",
            "unit-test gate (verify.sh)",
            3,
            "assertion failed: foo != bar",
            /* with_comment */ true,
            &GithubStateMap::default(),
            Some(&binding),
        )
        .await;
        assert_eq!(res, TransitionResult::Applied);

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(
            lines.len(),
            6,
            "label ensure + edit + comment, then 3 GraphQL: {calls}"
        );
        assert_eq!(
            lines[0],
            "label create status/blocked --repo acme/seam-blocked --color b60205 --force"
        );
        assert!(
            lines[1]
                .starts_with("issue edit 42 --repo acme/seam-blocked --add-label status/blocked"),
            "got: {}",
            lines[1]
        );
        assert_eq!(lines[2], "issue comment");
        assert!(
            lines[3].contains("repository(owner: $owner"),
            "{}",
            lines[3]
        );
        assert!(lines[4].contains("addProjectV2ItemById"), "{}", lines[4]);
        assert!(
            lines[5].contains("updateProjectV2ItemFieldValue")
                && lines[5].contains("-f option=opt-blocked"),
            "the Blocked-mapped OPTION ID rides the write: {}",
            lines[5]
        );
        assert!(
            !calls.contains("issue view")
                && !calls.contains("issue close")
                && !calls.contains("issue reopen"),
            "no close/reopen on Blocked: {calls}"
        );
    }

    #[tokio::test]
    async fn board_sink_creates_a_feat_card_and_returns_its_key() {
        let store = fresh_store().await;
        let here = std::env::temp_dir();
        let r = TaskSink::Board
            .create_feature(
                &ctx(&store, &here, None),
                &NewFeature {
                    title: "Add OAuth login".into(),
                    body: Some("user can sign in with Google".into()),
                    labels: vec![],
                },
            )
            .await
            .expect("board sink must create a card");

        assert_eq!(r.provider, "board");
        assert!(r.url.is_none(), "board cards have no external url");
        assert!(
            r.id.starts_with("AG-"),
            "feature ref id must be the board key, got {}",
            r.id
        );

        let items = store.list_board_items().await.unwrap();
        let card = items
            .iter()
            .find(|c| c.key == r.id)
            .expect("created card must be listable");
        assert_eq!(card.title, "Add OAuth login");
        assert_eq!(card.lbl.as_deref(), Some("feat"));
        assert_eq!(card.status, "todo");
    }

    #[tokio::test]
    async fn board_sink_nests_feature_under_parent_goal() {
        let store = fresh_store().await;
        let here = std::env::temp_dir();
        let goal = store
            .create_board_item(NewBoardItem {
                title: "Ship auth".into(),
                body: None,
                lbl: Some("goal".into()),
                status: Some("todo".into()),
                workdir: None,
                parent_goal_id: None,
                tool: None,
                model: None,
                session_id: None,
                priority: None,
            })
            .await
            .unwrap();

        let r = TaskSink::Board
            .create_feature(
                &ctx(&store, &here, Some(goal.id)),
                &NewFeature {
                    title: "Login screen".into(),
                    body: None,
                    labels: vec![],
                },
            )
            .await
            .unwrap();

        let items = store.list_board_items().await.unwrap();
        let card = items.iter().find(|c| c.key == r.id).unwrap();
        assert_eq!(
            card.parent_goal_id,
            Some(goal.id),
            "feature must nest under its goal"
        );
    }

    /// The GitHub sink's live path needs an authenticated `gh` inside a real
    /// GitHub repo. Run with `--ignored` from such a checkout to exercise it.
    #[tokio::test]
    #[ignore = "requires authenticated gh inside a real GitHub repo"]
    async fn github_sink_creates_a_real_issue() {
        let store = fresh_store().await;
        let cwd = std::env::current_dir().unwrap();
        let r = TaskSink::Github
            .create_feature(
                &ctx(&store, &cwd, None),
                &NewFeature {
                    title: "agentum 011b smoke test".into(),
                    body: Some("created by github_sink_creates_a_real_issue".into()),
                    labels: vec![],
                },
            )
            .await
            .expect("gh issue create must succeed in a real repo");
        assert_eq!(r.provider, "github");
        assert!(r.url.is_some());
    }
}
