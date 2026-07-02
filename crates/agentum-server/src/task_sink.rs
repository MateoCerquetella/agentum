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
                let body = feature.body.clone().unwrap_or_default();
                let mut cmd = tokio::process::Command::new("gh");
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
    ReadyToTest,
    Done,
}

/// Parse a wire-format phase string (`todo` / `in_progress` / `ready_to_test` /
/// `done`) into a [`TrackerPhase`]. Pure; `None` for anything else — the MCP
/// `agentum_report_status` tool (spec 005 F4) treats that as a caller bug, not
/// a tracker hiccup.
pub fn parse_tracker_phase(s: &str) -> Option<TrackerPhase> {
    match s {
        "todo" => Some(TrackerPhase::Todo),
        "in_progress" => Some(TrackerPhase::InProgress),
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
    /// Nothing to do (provider has no such concept, or no external tracker).
    Skipped(String),
}

/// The board column for a phase. Pure so the mapping is unit-tested. The board
/// ships `todo`/`doing`/`review`/`done` columns (see board_column_rule tests).
fn board_status_for(phase: TrackerPhase) -> &'static str {
    match phase {
        TrackerPhase::Todo => "todo",
        TrackerPhase::InProgress => "doing",
        TrackerPhase::ReadyToTest => "review",
        TrackerPhase::Done => "done",
    }
}

/// Canonical, harness-owned status labels with fixed colors (spec 004 D3).
/// NOT `.github/labels.sh`'s `status/qa*` set — that is the human-QA lifecycle
/// (architecture C4); the transition never touches foreign `status/*` labels.
const GITHUB_STATUS_LABELS: [(TrackerPhase, &str, &str); 4] = [
    (TrackerPhase::Todo, "status/todo", "ededed"),
    (TrackerPhase::InProgress, "status/in-progress", "1d76db"),
    (TrackerPhase::ReadyToTest, "status/ready-to-test", "fbca04"),
    (TrackerPhase::Done, "status/done", "0e8a16"),
];

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
fn github_status_color(phase: TrackerPhase) -> &'static str {
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
    pub ready_to_test: String,
    pub done: String,
}

impl Default for GithubStateMap {
    /// The canonical four from [`GITHUB_STATUS_LABELS`], via the
    /// [`github_status_label`] accessor so defaults and table can't drift.
    fn default() -> Self {
        Self {
            todo: github_status_label(TrackerPhase::Todo).into(),
            in_progress: github_status_label(TrackerPhase::InProgress).into(),
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
            TrackerPhase::ReadyToTest => &self.ready_to_test,
            TrackerPhase::Done => &self.done,
        }
    }

    /// The configured names in canonical phase order (may contain duplicates
    /// when a user maps two phases to one name — callers dedupe by name).
    fn labels(&self) -> [&str; 4] {
        [
            &self.todo,
            &self.in_progress,
            &self.ready_to_test,
            &self.done,
        ]
    }
}

/// Idempotent ensure-create: `--force` updates an existing label's color to
/// canonical instead of failing. One argv token per value — never a shell.
fn gh_label_ensure_argv<'a>(name: &'a str, slug: &'a str, color: &'a str) -> [&'a str; 8] {
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
pub async fn apply_tracker_transition(
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
        // canonical `status/*` label (spec 004 D3). `Done` is label-only — the
        // issue stays open; closing remains the PR's `Closes #N` job (D1).
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
            Ok(github_transition_with(&gh_bin(), &slug, &number, phase, &map).await)
        }
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
            github_status_label(TrackerPhase::ReadyToTest),
            github_status_label(TrackerPhase::Done),
        ];
        assert_eq!(
            labels,
            [
                "status/todo",
                "status/in-progress",
                "status/ready-to-test",
                "status/done"
            ]
        );
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), 4, "labels must be pairwise distinct");
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

    /// Spec 004 (C4 invariant at argv level): one `gh issue edit` adds the
    /// target label and removes exactly the OTHER three canonical labels —
    /// the target is never removed, and no non-canonical name (e.g. this
    /// repo's own `status/qa*` human-QA labels) ever appears in the argv.
    ///
    /// Spec 005 F5 regression pin: with the DEFAULT map the argv must stay
    /// **byte-identical** to what spec 004 shipped — same tokens, same order.
    /// The `expected` literals below were captured against the pre-F5
    /// (map-less) builder; do not regenerate them from the code under test.
    #[test]
    fn gh_set_status_label_argv_adds_one_removes_exactly_the_other_three() {
        let all_phases = [
            TrackerPhase::Todo,
            TrackerPhase::InProgress,
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
                    "status/ready-to-test",
                    "--remove-label",
                    "status/done",
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
                    "status/ready-to-test",
                    "--remove-label",
                    "status/done",
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
                    "status/done",
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
                    "status/ready-to-test",
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
            // Tail: a `--remove-label <l>` pair for each of the other three.
            let removed: Vec<&str> = argv[7..]
                .chunks(2)
                .map(|pair| {
                    assert_eq!(pair[0], "--remove-label");
                    pair[1]
                })
                .collect();
            assert_eq!(removed.len(), 3, "exactly three labels removed");
            assert!(
                !removed.contains(&target),
                "the target label must never be removed"
            );
            for r in &removed {
                assert!(
                    GITHUB_STATUS_LABELS.iter().any(|(_, name, _)| name == r),
                    "non-canonical label {r} in the remove set (C4 violation)"
                );
            }
            for (p, name, _) in GITHUB_STATUS_LABELS.iter() {
                if *p != phase {
                    assert!(removed.contains(name), "{name} missing from remove set");
                }
            }
        }
    }

    /// Spec 005 F5: the default map IS the canonical `GITHUB_STATUS_LABELS`
    /// name set, and `label_for` agrees with the const-table accessor.
    #[test]
    fn github_state_map_defaults_are_canonical() {
        let m = GithubStateMap::default();
        assert_eq!(m.todo, "status/todo");
        assert_eq!(m.in_progress, "status/in-progress");
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
                "qa-ready",
                "--remove-label",
                "shipped",
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
                ["status/todo", "status/done"],
                "the shared target must be absent from its own remove list"
            );
        }
        // Shared name NOT the target → removed once, not twice.
        let argv = gh_set_status_label_argv("42", "o/r", TrackerPhase::Done, &map);
        let removed: Vec<&str> = argv[7..].chunks(2).map(|pair| pair[1]).collect();
        assert_eq!(removed, ["status/todo", "active"]);
    }

    /// Spec 005 F5 (§6 item 6): a fake `gh` logs the full custom-map
    /// transition — 4 ensure-creates carrying the CUSTOM names with the
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
        assert_eq!(lines.len(), 5, "4 ensure-creates + 1 edit, got: {calls}");
        assert_eq!(
            lines[..4],
            [
                "label create triage --repo owner/repo --color ededed --force",
                "label create wip --repo owner/repo --color 1d76db --force",
                "label create qa-ready --repo owner/repo --color fbca04 --force",
                "label create shipped --repo owner/repo --color 0e8a16 --force",
            ]
        );
        assert_eq!(
            lines[4],
            "issue edit 42 --repo owner/repo --add-label qa-ready \
             --remove-label triage --remove-label wip --remove-label shipped"
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
        assert_eq!(lines.len(), 4, "3 deduped ensure-creates + 1 edit: {calls}");
        assert_eq!(
            lines[..3],
            [
                "label create status/todo --repo owner/repo --color ededed --force",
                // InProgress comes first in canonical order, so its color wins.
                "label create active --repo owner/repo --color 1d76db --force",
                "label create status/done --repo owner/repo --color 0e8a16 --force",
            ]
        );
    }

    /// Spec 005 F4: the wire-format phase parser accepts exactly the four
    /// pipeline phases and rejects everything else (case-sensitive, no aliases).
    #[test]
    fn parse_tracker_phase_accepts_the_four_and_rejects_junk() {
        assert_eq!(parse_tracker_phase("todo"), Some(TrackerPhase::Todo));
        assert_eq!(
            parse_tracker_phase("in_progress"),
            Some(TrackerPhase::InProgress)
        );
        assert_eq!(
            parse_tracker_phase("ready_to_test"),
            Some(TrackerPhase::ReadyToTest)
        );
        assert_eq!(parse_tracker_phase("done"), Some(TrackerPhase::Done));
        for junk in ["", "Todo", "DONE", "in-progress", "ready to test", "qa"] {
            assert_eq!(parse_tracker_phase(junk), None, "{junk:?} must be rejected");
        }
    }

    #[test]
    fn board_status_mapping_covers_all_phases() {
        assert_eq!(board_status_for(TrackerPhase::Todo), "todo");
        assert_eq!(board_status_for(TrackerPhase::InProgress), "doing");
        assert_eq!(board_status_for(TrackerPhase::ReadyToTest), "review");
        assert_eq!(board_status_for(TrackerPhase::Done), "done");
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

        let res = apply_tracker_transition(&store, "board", &r.id, None, TrackerPhase::InProgress)
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
        let res = apply_tracker_transition(&store, "board", "AG-9999", None, TrackerPhase::Done)
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
        let res = apply_tracker_transition(&store, "github", "42", None, TrackerPhase::Done)
            .await
            .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
        // Blank and unparseable (a /pull/ link) URLs are skips too.
        let res = apply_tracker_transition(&store, "github", "42", Some("  "), TrackerPhase::Done)
            .await
            .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
        let res = apply_tracker_transition(
            &store,
            "github",
            "42",
            Some("https://github.com/o/r/pull/42"),
            TrackerPhase::Done,
        )
        .await
        .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
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
        // 4 ensure-creates then the single issue edit, in that order.
        assert_eq!(lines.len(), 5, "expected 5 gh invocations, got: {calls}");
        for line in &lines[..4] {
            assert!(
                line.starts_with("label create status/"),
                "expected an ensure-create, got: {line}"
            );
            assert!(line.ends_with("--force"));
        }
        assert!(
            lines[4].starts_with("issue edit 42 --repo owner/repo --add-label status/in-progress"),
            "last call must be the issue edit, got: {}",
            lines[4]
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
