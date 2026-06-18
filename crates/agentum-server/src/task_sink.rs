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
}

/// Where a created feature landed. `id` is the provider's stable handle (board
/// key like `AG-12`, a GitHub issue number, a Linear identifier) — the
/// chat-to-features pipeline reuses it as the harness feature id so
/// `$HARNESS_FEATURE_ID` in `verify.sh` points back at the real tracker item.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    // Linear,  // spec 011c — GraphQL `issueCreate`
}

impl TaskSink {
    /// Stable provider id, surfaced to callers and stamped onto [`FeatureRef`].
    pub fn provider(self) -> &'static str {
        match self {
            TaskSink::Board => "board",
            TaskSink::Github => "github",
        }
    }

    /// Decide the destination from which providers are available. Pure policy so
    /// it's unit-testable; the IO that discovers availability lives in
    /// [`TaskSink::select`].
    ///
    /// An external manager is the source of truth when configured; the internal
    /// board is the agnostic fallback. (Linear precedence lands with 011c.)
    pub fn pick_provider(github_available: bool) -> TaskSink {
        if github_available {
            TaskSink::Github
        } else {
            TaskSink::Board
        }
    }

    /// Resolve the destination for a goal's `workdir` by probing what's
    /// configured, then delegating to [`TaskSink::pick_provider`].
    pub async fn select(workdir: &Path) -> TaskSink {
        TaskSink::pick_provider(github_ready(workdir).await)
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
                let output = tokio::process::Command::new("gh")
                    .args(gh_create_argv(&feature.title, &body))
                    .current_dir(ctx.workdir)
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
        }
    }
}

/// `gh issue create` argv for a non-interactive create. Kept as a pure helper so
/// the argument shape is unit-tested without spawning a process.
fn gh_create_argv<'a>(title: &'a str, body: &'a str) -> [&'a str; 6] {
    ["issue", "create", "--title", title, "--body", body]
}

/// Parse the issue URL `gh issue create` prints to stdout into a [`FeatureRef`].
/// The number (after `/issues/`) becomes the harness feature id; the full URL is
/// surfaced to the user.
fn parse_gh_issue_url(stdout: &str) -> anyhow::Result<FeatureRef> {
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
        }
    }

    #[test]
    fn pick_provider_prefers_github_else_board() {
        // External manager = truth when configured; board = agnostic fallback.
        assert_eq!(TaskSink::pick_provider(true), TaskSink::Github);
        assert_eq!(TaskSink::pick_provider(false), TaskSink::Board);
    }

    #[test]
    fn provider_ids_are_stable() {
        assert_eq!(TaskSink::Board.provider(), "board");
        assert_eq!(TaskSink::Github.provider(), "github");
    }

    #[test]
    fn gh_create_argv_is_noninteractive() {
        let argv = gh_create_argv("My title", "My body");
        assert_eq!(
            argv,
            ["issue", "create", "--title", "My title", "--body", "My body"]
        );
    }

    #[test]
    fn parse_gh_issue_url_extracts_number_and_url() {
        // gh prints a banner line or two then the URL last.
        let out = "Creating issue in owner/repo\nhttps://github.com/owner/repo/issues/42\n";
        let r = parse_gh_issue_url(out).unwrap();
        assert_eq!(r.provider, "github");
        assert_eq!(r.id, "42");
        assert_eq!(r.url.as_deref(), Some("https://github.com/owner/repo/issues/42"));
    }

    #[test]
    fn parse_gh_issue_url_errors_without_url() {
        assert!(parse_gh_issue_url("nothing useful here\n").is_err());
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
                },
            )
            .await
            .expect("gh issue create must succeed in a real repo");
        assert_eq!(r.provider, "github");
        assert!(r.url.is_some());
    }
}
