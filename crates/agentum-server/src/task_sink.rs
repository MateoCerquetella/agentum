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

use std::path::Path;

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

/// Drive a created feature's tracker item to `phase`, dispatching on the provider
/// recorded when the feature was created. **Best-effort by contract**: returns
/// `Ok(Skipped)` for providers/states that don't apply and only `Err` for a real
/// transport failure the caller should log — never a reason to halt the harness.
///
/// `tracker_id` is the provider's stable handle (board key, Linear identifier,
/// GitHub issue number) — the same value stored as the harness feature id.
pub async fn apply_tracker_transition(
    store: &Store,
    provider: &str,
    tracker_id: &str,
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
        // GitHub Issues has no built-in workflow column; a label/comment sync is a
        // future refinement (spec 012 follow-up). No-op for now, logged by caller.
        "github" => Ok(TransitionResult::Skipped(
            "github issue state sync not implemented".into(),
        )),
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

        let res = apply_tracker_transition(&store, "board", &r.id, TrackerPhase::InProgress)
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
        let res = apply_tracker_transition(&store, "board", "AG-9999", TrackerPhase::Done)
            .await
            .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
    }

    #[tokio::test]
    async fn github_transition_is_a_logged_noop() {
        let store = fresh_store().await;
        let res = apply_tracker_transition(&store, "github", "42", TrackerPhase::Done)
            .await
            .unwrap();
        assert!(matches!(res, TransitionResult::Skipped(_)));
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
