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
//! dependency (`async-trait`). v1 ships the internal board only; GitHub Issues
//! (spec 011b) and Linear (spec 011c) slot in as new variants.

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

/// The configured task destination. v1: internal board only. The internal board
/// is also the agnostic *fallback* — it is the source of truth whenever no
/// external manager is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSink {
    /// agentum's own kanban board (`board_items`).
    Board,
    // Github,  // spec 011b — `gh issue create`
    // Linear,  // spec 011c — GraphQL `issueCreate`
}

impl TaskSink {
    /// Stable provider id, surfaced to callers and stamped onto [`FeatureRef`].
    pub fn provider(self) -> &'static str {
        match self {
            TaskSink::Board => "board",
        }
    }

    /// Pick the destination from what the user has configured. v1 always
    /// resolves to [`TaskSink::Board`] — the agnostic fallback and the source of
    /// truth when no external manager exists. GitHub (011b) and Linear (011c)
    /// will branch here off `/api/preflight/check` detection; kept as a single
    /// honest return until those sinks exist (no speculative wiring).
    pub fn select() -> TaskSink {
        TaskSink::Board
    }

    /// Create one feature in the backing task manager. Returns a [`FeatureRef`]
    /// the caller can surface; an `Err` must be propagated, never swallowed —
    /// the pipeline reports per-feature failures rather than silently dropping
    /// them (spec 011 risk: no silent partial state).
    ///
    /// `parent_ref` groups the feature under its originating goal: for the board
    /// it sets `parent_goal_id` so the card nests under the goal. Providers that
    /// don't model hierarchy ignore it.
    pub async fn create_feature(
        self,
        store: &Store,
        parent_ref: Option<i64>,
        feature: &NewFeature,
    ) -> anyhow::Result<FeatureRef> {
        match self {
            TaskSink::Board => {
                // A feature is a `feat` card in `todo`; the board's `todo` gate
                // requires only Title + Lbl, both present here. The card mirrors
                // the feature on the kanban view; when later moved to `doing` the
                // existing board flow spawns its agent session.
                let item = store
                    .create_board_item(NewBoardItem {
                        title: feature.title.clone(),
                        body: feature.body.clone(),
                        lbl: Some("feat".into()),
                        status: Some("todo".into()),
                        workdir: None,
                        parent_goal_id: parent_ref,
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
        }
    }
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

    #[test]
    fn board_is_the_default_selection() {
        assert_eq!(TaskSink::select(), TaskSink::Board);
        assert_eq!(TaskSink::Board.provider(), "board");
    }

    #[tokio::test]
    async fn board_sink_creates_a_feat_card_and_returns_its_key() {
        let store = fresh_store().await;
        let r = TaskSink::Board
            .create_feature(
                &store,
                None,
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

        // The card is really on the board as a `feat` in `todo`.
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
        // A goal card to parent the feature under.
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
                &store,
                Some(goal.id),
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
}
