//! `/api/board/links` — directed edges between board items (kinds:
//! `parent_of`, `blocks`). Used by the planner CLI shim to declare
//! dependencies (`--blocks foo`) and by Phase 3's column-rule gate to
//! refuse `doing` transitions when an inbound blocks-edge is unfinished.
//! See `.planning/phases/01-goal-cards-planner-slice/01-CONTEXT.md` D-06.

use std::str::FromStr;

use agentum_core::{BoardItem, BoardLink, Event, LinkKind};
use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/links", post(create).get(list))
        .route("/api/board/links/{from}/{to}/{kind}", delete(delete_one))
}

/// Two body shapes for POST /api/board/links:
/// - `Direct`: caller already knows both card ids
/// - `Symbolic`: caller supplies keys relative to a parent goal; the
///   daemon resolves them against children of that goal
#[derive(Deserialize)]
#[serde(untagged)]
enum CreateLinkBody {
    Direct {
        from_card_id: i64,
        to_card_id: i64,
        kind: String,
    },
    Symbolic {
        parent_goal_id: i64,
        from_key: String,
        to_key: String,
        kind: String,
    },
}

#[derive(Deserialize)]
struct ListQuery {
    goal: i64,
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateLinkBody>,
) -> Result<(StatusCode, Json<BoardLink>), ApiError> {
    let (from, to, kind_str) = match body {
        CreateLinkBody::Direct {
            from_card_id,
            to_card_id,
            kind,
        } => (from_card_id, to_card_id, kind),
        CreateLinkBody::Symbolic {
            parent_goal_id,
            from_key,
            to_key,
            kind,
        } => {
            // T-03-02: reject keys with chars outside [a-zA-Z0-9_-] before
            // they reach the title-LIKE pattern; prevents wildcard injection.
            validate_symbolic_key(&from_key)?;
            validate_symbolic_key(&to_key)?;

            let children = state.store.list_children_of_goal(parent_goal_id).await?;
            let from_id = resolve_key(&children, &from_key)
                .ok_or_else(|| ApiError::BadRequest(format!("unknown sibling key: {from_key}")))?;
            let to_id = resolve_key(&children, &to_key)
                .ok_or_else(|| ApiError::BadRequest(format!("unknown sibling key: {to_key}")))?;
            (from_id, to_id, kind)
        }
    };

    let kind = LinkKind::from_str(&kind_str)
        .map_err(|_| ApiError::BadRequest(format!("unknown link kind: {kind_str}")))?;

    let link = state.store.add_board_link(from, to, kind).await?;

    let _ = state.bus.send(
        Event::new("board.link.created")
            .with_payload(json!({"from": from, "to": to, "kind": kind.as_str()})),
    );

    Ok((StatusCode::CREATED, Json(link)))
}

/// Validate that a symbolic key contains only safe characters.
/// Keys outside [a-zA-Z0-9_-] are rejected before any SQL to prevent
/// wildcard injection into LIKE patterns (T-03-02).
fn validate_symbolic_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || key.len() > 64 {
        return Err(ApiError::BadRequest(format!("invalid key length: {key}")));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::BadRequest(format!(
            "invalid key (must match [a-zA-Z0-9_-]+): {key}"
        )));
    }
    Ok(())
}

/// Resolve a symbolic key against a list of sibling board items.
/// The CLI shim (plan 01-05) stores the symbolic key as a sidecar in
/// the card's `body` field using the convention `key: <key>\n\n<body>`
/// so this route can parse it without a schema change. Match on the
/// first line of body exactly (prefix `key: `).
fn resolve_key(children: &[BoardItem], key: &str) -> Option<i64> {
    let needle = format!("key: {key}");
    children.iter().find_map(|c| {
        if let Some(body) = c.body.as_ref() {
            if body.lines().next().is_some_and(|l| l.trim() == needle) {
                return Some(c.id);
            }
        }
        None
    })
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<BoardLink>>, ApiError> {
    let links = state.store.list_board_links_for_goal(q.goal).await?;
    Ok(Json(links))
}

async fn delete_one(
    State(state): State<AppState>,
    Path((from, to, kind)): Path<(i64, i64, String)>,
) -> Result<StatusCode, ApiError> {
    let kind = LinkKind::from_str(&kind)
        .map_err(|_| ApiError::BadRequest(format!("unknown kind: {kind}")))?;
    let removed = state.store.delete_board_link(from, to, kind).await?;
    if removed {
        let _ = state.bus.send(
            Event::new("board.link.deleted")
                .with_payload(json!({"from": from, "to": to, "kind": kind.as_str()})),
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!(
            "board link {from} -{kind:?}-> {to}"
        )))
    }
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for the board-links CRUD surface.
    //! Uses the same in-process AppState harness as board.rs and
    //! board_rules.rs tests — no real tmux or HTTP server.
    //!
    //! Auth middleware is verified at the lib.rs::router() merge site
    //! (top-level `require_token` layer) — the in-process test harness
    //! bypasses middleware by calling handlers directly, so the
    //! "unauthenticated request" scenario is documented as a skip below.

    use super::*;
    use agentum_core::NewBoardItem;
    use agentum_store::Store;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        AppState {
            store: Arc::new(store),
            bus,
            started_at: std::time::Instant::now(),
            version: "test",
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                8,
                std::time::Duration::from_secs(60),
            )),
            cert_fingerprint: Arc::new(String::new()),
            transcripts: crate::TranscriptStore::new(broadcast::channel(16).0),
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
            events_ws_clients: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Helper: create a board item with title + lbl in todo.
    async fn make_item(state: &AppState, title: &str, parent_goal_id: Option<i64>) -> i64 {
        state
            .store
            .create_board_item(NewBoardItem {
                title: title.into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id,
            })
            .await
            .unwrap()
            .id
    }

    /// Helper: create a board item with a symbolic key stored in the body
    /// prefix per the `key: <key>\n\n<body>` convention (plan 01-05).
    async fn make_keyed_item(state: &AppState, title: &str, key: &str, parent_goal_id: i64) -> i64 {
        state
            .store
            .create_board_item(NewBoardItem {
                title: title.into(),
                body: Some(format!("key: {key}\n\nsome body")),
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: Some(parent_goal_id),
            })
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn create_link_direct_ids() {
        let state = fresh_state().await;
        let a = make_item(&state, "card-a", None).await;
        let b = make_item(&state, "card-b", None).await;
        let mut rx = state.bus.subscribe();

        let (code, link) = create(
            State(state.clone()),
            Json(CreateLinkBody::Direct {
                from_card_id: a,
                to_card_id: b,
                kind: "blocks".into(),
            }),
        )
        .await
        .expect("direct create must succeed");

        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(link.0.from_card_id, a);
        assert_eq!(link.0.to_card_id, b);
        assert_eq!(link.0.kind, LinkKind::Blocks);

        // Event must fire on the bus.
        let ev = rx.recv().await.expect("board.link.created event");
        assert_eq!(ev.kind, "board.link.created");
        assert_eq!(ev.payload["from"], a);
        assert_eq!(ev.payload["to"], b);
        assert_eq!(ev.payload["kind"], "blocks");

        // Persisted — list confirms.
        let links = list(State(state), Query(ListQuery { goal: a }))
            .await
            .unwrap()
            .0;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from_card_id, a);
    }

    #[tokio::test]
    async fn create_link_by_symbolic_key() {
        let state = fresh_state().await;
        let goal_id = make_item(&state, "goal", None).await;
        let _schema_id = make_keyed_item(&state, "schema card", "schema", goal_id).await;
        let _types_id = make_keyed_item(&state, "types card", "types", goal_id).await;

        let (code, _link) = create(
            State(state.clone()),
            Json(CreateLinkBody::Symbolic {
                parent_goal_id: goal_id,
                from_key: "types".into(),
                to_key: "schema".into(),
                kind: "blocks".into(),
            }),
        )
        .await
        .expect("symbolic key create must succeed");

        assert_eq!(code, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_link_unknown_symbolic_key_returns_400() {
        let state = fresh_state().await;
        let goal_id = make_item(&state, "goal", None).await;
        let _child = make_keyed_item(&state, "auth card", "auth", goal_id).await;

        let err = create(
            State(state),
            Json(CreateLinkBody::Symbolic {
                parent_goal_id: goal_id,
                from_key: "nope".into(),
                to_key: "auth".into(),
                kind: "blocks".into(),
            }),
        )
        .await
        .expect_err("unknown from_key must be 400");

        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("unknown sibling key: nope"))
        );
    }

    #[tokio::test]
    async fn create_link_invalid_symbolic_key_charset_returns_400() {
        let state = fresh_state().await;
        let goal_id = make_item(&state, "goal", None).await;

        let err = create(
            State(state),
            Json(CreateLinkBody::Symbolic {
                parent_goal_id: goal_id,
                from_key: "../etc/passwd".into(),
                to_key: "auth".into(),
                kind: "blocks".into(),
            }),
        )
        .await
        .expect_err("path-traversal key must be rejected");

        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("invalid key")));
    }

    #[tokio::test]
    async fn create_link_unknown_kind_returns_400() {
        let state = fresh_state().await;
        let a = make_item(&state, "card-a", None).await;
        let b = make_item(&state, "card-b", None).await;

        let err = create(
            State(state),
            Json(CreateLinkBody::Direct {
                from_card_id: a,
                to_card_id: b,
                kind: "garbage".into(),
            }),
        )
        .await
        .expect_err("unknown kind must 400");

        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("unknown link kind")));
    }

    #[tokio::test]
    async fn create_duplicate_link_returns_409() {
        let state = fresh_state().await;
        let a = make_item(&state, "card-a", None).await;
        let b = make_item(&state, "card-b", None).await;

        let _ = create(
            State(state.clone()),
            Json(CreateLinkBody::Direct {
                from_card_id: a,
                to_card_id: b,
                kind: "blocks".into(),
            }),
        )
        .await
        .expect("first create");

        let err = create(
            State(state),
            Json(CreateLinkBody::Direct {
                from_card_id: a,
                to_card_id: b,
                kind: "blocks".into(),
            }),
        )
        .await
        .expect_err("duplicate must be 409");

        assert!(matches!(err, ApiError::Conflict(_)));
    }

    #[tokio::test]
    async fn list_links_filters_by_goal() {
        // goal → child_a (via from_card_id=goal)
        // child_a → child_b (via from_card_id=child_a)
        // GET ?goal=goal_id must return only the first link.
        let state = fresh_state().await;
        let goal_id = make_item(&state, "goal", None).await;
        let child_a = make_item(&state, "child-a", Some(goal_id)).await;
        let child_b = make_item(&state, "child-b", Some(goal_id)).await;

        let _ = create(
            State(state.clone()),
            Json(CreateLinkBody::Direct {
                from_card_id: goal_id,
                to_card_id: child_a,
                kind: "parent_of".into(),
            }),
        )
        .await
        .unwrap();
        let _ = create(
            State(state.clone()),
            Json(CreateLinkBody::Direct {
                from_card_id: child_a,
                to_card_id: child_b,
                kind: "blocks".into(),
            }),
        )
        .await
        .unwrap();

        let links = list(State(state), Query(ListQuery { goal: goal_id }))
            .await
            .unwrap()
            .0;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from_card_id, goal_id);
        assert_eq!(links[0].to_card_id, child_a);
    }

    #[tokio::test]
    async fn delete_link_returns_204_then_404() {
        let state = fresh_state().await;
        let a = make_item(&state, "card-a", None).await;
        let b = make_item(&state, "card-b", None).await;

        let _ = create(
            State(state.clone()),
            Json(CreateLinkBody::Direct {
                from_card_id: a,
                to_card_id: b,
                kind: "blocks".into(),
            }),
        )
        .await
        .unwrap();

        // First delete: 204.
        let code = delete_one(State(state.clone()), Path((a, b, "blocks".into())))
            .await
            .expect("first delete must be 204");
        assert_eq!(code, StatusCode::NO_CONTENT);

        // Second delete: 404.
        let err = delete_one(State(state), Path((a, b, "blocks".into())))
            .await
            .expect_err("second delete must 404");
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    /// Auth middleware is verified at the lib.rs::router() merge site via the
    /// top-level `require_token` layer — the in-process test harness calls
    /// handlers directly and bypasses middleware. Testing 401 here would
    /// require spinning up a full axum server, which is deferred to the
    /// end-to-end integration tests in plan 01-08.
    #[test]
    fn unauthenticated_request_verified_at_router_merge() {
        // Documented skip — see comment above.
    }
}
