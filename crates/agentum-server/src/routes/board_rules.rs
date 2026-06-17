//! `/api/board/rules` — per-server overrides of the compile-time
//! required-field matrix. See spec
//! `.planning/specs/2026-05-20-board-column-rules-overrides.md`.
//!
//! Three handlers: GET (merged view of const + DB overrides), PUT
//! (upsert one column's rule), DELETE (drop one column's rule). The
//! gate (`routes::board::enforce_transition`) consults the resolved
//! result via `crate::rules::resolve_required_fields`.

use std::collections::BTreeMap;

use agentum_core::{Event, RequiredField};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/rules", get(list))
        .route("/api/board/rules/{column}", put(upsert).delete(delete))
}

async fn list(
    State(state): State<AppState>,
) -> Result<Json<BTreeMap<String, Vec<RequiredField>>>, ApiError> {
    let merged = crate::rules::merged_rule_matrix(&state.store).await?;
    Ok(Json(merged))
}

#[derive(Debug, Deserialize)]
struct UpsertBody {
    /// Wire-vocabulary strings (`"title"`, `"lbl"`, etc.). Empty array
    /// is valid — it persists as the explicit "no gate" choice.
    required_fields: Vec<String>,
}

async fn upsert(
    State(state): State<AppState>,
    Path(column): Path<String>,
    Json(body): Json<UpsertBody>,
) -> Result<StatusCode, ApiError> {
    // Parse field names through the typed enum so the store gets a
    // validated input. First unknown name → 400 with the AC-pinned
    // body shape `{"error": "unknown field: <name>"}` (handled by
    // ApiError::BadRequest's default envelope).
    let mut parsed: Vec<RequiredField> = Vec::with_capacity(body.required_fields.len());
    for name in &body.required_fields {
        match RequiredField::from_missing_key(name) {
            Some(f) => parsed.push(f),
            None => {
                return Err(ApiError::BadRequest(format!("unknown field: {name}")));
            }
        }
    }

    // Skip the write + event when the incoming rule is structurally
    // identical to what's already stored. Avoids spurious
    // `board.rules.updated` events from idempotent PUTs.
    if let Some(existing) = state.store.get_board_column_rule(&column).await?
        && existing == parsed
    {
        return Ok(StatusCode::OK);
    }

    state
        .store
        .upsert_board_column_rule(&column, &parsed)
        .await?;

    // Emit the wire-vocabulary strings (not the typed enum) so the
    // payload matches the GET response shape.
    let wire: Vec<&'static str> = parsed.iter().map(|f| f.as_missing_key()).collect();
    let _ = state
        .bus
        .send(Event::new("board.rules.updated").with_payload(json!({
            "column": column,
            "required_fields": wire,
        })));
    Ok(StatusCode::OK)
}

async fn delete(
    State(state): State<AppState>,
    Path(column): Path<String>,
) -> Result<StatusCode, ApiError> {
    let removed = state.store.delete_board_column_rule(&column).await?;
    if !removed {
        return Err(ApiError::NotFound(format!("board rule for {column}")));
    }
    let _ = state
        .bus
        .send(Event::new("board.rules.deleted").with_payload(json!({
            "column": column,
        })));
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    //! Handler-level tests covering all AC items for the rules CRUD,
    //! plus end-to-end checks that the gate honours overrides (loosen
    //! `doing`, gate a custom `review`, then DELETE to restore the
    //! previous behaviour).

    use super::*;
    use agentum_core::{BoardPatch, NewBoardItem};
    use agentum_store::Store;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use std::sync::Arc;
    use std::time::Duration;
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
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
        }
    }

    async fn err_status_and_body(err: ApiError) -> (StatusCode, serde_json::Value) {
        let resp = err.into_response();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn list_empty_db_returns_const_defaults() {
        // Empty DB: the three default columns appear with their slice-1
        // const values. No custom columns leak in.
        let state = fresh_state().await;
        let out = list(State(state)).await.unwrap();
        let map = out.0;
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get("todo"),
            Some(&vec![RequiredField::Title, RequiredField::Lbl])
        );
        assert_eq!(
            map.get("doing"),
            Some(&vec![
                RequiredField::Title,
                RequiredField::Lbl,
                RequiredField::Workdir,
                RequiredField::Tool,
                RequiredField::ClaimedBy,
            ])
        );
        assert_eq!(
            map.get("done"),
            Some(&vec![
                RequiredField::Title,
                RequiredField::Lbl,
                RequiredField::SessionOrComment,
            ])
        );
    }

    #[tokio::test]
    async fn list_returns_overrides_merged() {
        // Override `doing` and add a custom `review`. GET returns the
        // override for `doing` (not the const), the const for the
        // other two defaults, and the custom row on top.
        let state = fresh_state().await;
        state
            .store
            .upsert_board_column_rule("doing", &[RequiredField::Title, RequiredField::Lbl])
            .await
            .unwrap();
        state
            .store
            .upsert_board_column_rule(
                "review",
                &[RequiredField::Title, RequiredField::SessionOrComment],
            )
            .await
            .unwrap();

        let map = list(State(state)).await.unwrap().0;
        assert_eq!(
            map.get("doing"),
            Some(&vec![RequiredField::Title, RequiredField::Lbl])
        );
        assert_eq!(
            map.get("todo"),
            Some(&vec![RequiredField::Title, RequiredField::Lbl])
        );
        assert_eq!(
            map.get("review"),
            Some(&vec![RequiredField::Title, RequiredField::SessionOrComment])
        );
    }

    #[tokio::test]
    async fn put_happy_path_round_trips_through_get() {
        let state = fresh_state().await;
        let code = upsert(
            State(state.clone()),
            Path("doing".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into(), "lbl".into()],
            }),
        )
        .await
        .unwrap();
        assert_eq!(code, StatusCode::OK);

        let map = list(State(state)).await.unwrap().0;
        assert_eq!(
            map.get("doing"),
            Some(&vec![RequiredField::Title, RequiredField::Lbl])
        );
    }

    #[tokio::test]
    async fn put_unknown_field_returns_400_with_named_field() {
        // Spec AC pins the body to `{"error": "unknown field: wat"}`.
        let state = fresh_state().await;
        let err = upsert(
            State(state),
            Path("doing".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into(), "wat".into()],
            }),
        )
        .await
        .expect_err("unknown field must be rejected");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "unknown field: wat");
    }

    #[tokio::test]
    async fn put_empty_array_is_valid_and_passthrough() {
        // Spec decision 3: empty array is the explicit "no gate"
        // configuration. GET should show `doing: []`; subsequent gate
        // evaluations skip every requirement.
        let state = fresh_state().await;
        let code = upsert(
            State(state.clone()),
            Path("doing".into()),
            Json(UpsertBody {
                required_fields: vec![],
            }),
        )
        .await
        .unwrap();
        assert_eq!(code, StatusCode::OK);

        let map = list(State(state.clone())).await.unwrap().0;
        assert_eq!(map.get("doing"), Some(&vec![]));

        // Gate passthrough: POST a `doing` row with literally nothing
        // but a title — would fail under the slice-1 const, passes now.
        let res = crate::routes::board::tests_helpers_create(
            state,
            NewBoardItem {
                title: "no gate".into(),
                body: None,
                status: Some("doing".into()),
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            },
        )
        .await;
        assert!(res.is_ok(), "empty-rule doing must passthrough");
    }

    #[tokio::test]
    async fn delete_happy_path_restores_const_default() {
        // Custom column: present after PUT, absent after DELETE.
        // Default column: const value comes back after DELETE.
        let state = fresh_state().await;
        upsert(
            State(state.clone()),
            Path("review".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into()],
            }),
        )
        .await
        .unwrap();
        upsert(
            State(state.clone()),
            Path("doing".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into()],
            }),
        )
        .await
        .unwrap();

        let code = delete(State(state.clone()), Path("review".into()))
            .await
            .unwrap();
        assert_eq!(code, StatusCode::OK);
        let code = delete(State(state.clone()), Path("doing".into()))
            .await
            .unwrap();
        assert_eq!(code, StatusCode::OK);

        let map = list(State(state)).await.unwrap().0;
        // review (custom) is absent now.
        assert!(!map.contains_key("review"));
        // doing (default) snaps back to the slice-1 const.
        assert_eq!(
            map.get("doing"),
            Some(&vec![
                RequiredField::Title,
                RequiredField::Lbl,
                RequiredField::Workdir,
                RequiredField::Tool,
                RequiredField::ClaimedBy,
            ])
        );
    }

    #[tokio::test]
    async fn delete_missing_returns_404() {
        let state = fresh_state().await;
        let err = delete(State(state), Path("ghost".into()))
            .await
            .expect_err("deleting nonexistent rule must 404");
        let (status, _body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn idempotent_put_returns_200_both_times() {
        let state = fresh_state().await;
        let body = || UpsertBody {
            required_fields: vec!["title".into(), "lbl".into()],
        };
        let code1 = upsert(State(state.clone()), Path("doing".into()), Json(body()))
            .await
            .unwrap();
        let code2 = upsert(State(state.clone()), Path("doing".into()), Json(body()))
            .await
            .unwrap();
        assert_eq!(code1, StatusCode::OK);
        assert_eq!(code2, StatusCode::OK);
    }

    #[tokio::test]
    async fn integration_loosen_doing_then_restore() {
        // Full happy-path integration: PUT a loosened rule on `doing`,
        // POST a card satisfying only the loosened set, assert 200.
        // DELETE the rule, POST the same shape, assert 400.
        let state = fresh_state().await;
        upsert(
            State(state.clone()),
            Path("doing".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into(), "lbl".into()],
            }),
        )
        .await
        .unwrap();

        let ok = crate::routes::board::tests_helpers_create(
            state.clone(),
            NewBoardItem {
                title: "no workdir needed".into(),
                body: None,
                status: Some("doing".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            },
        )
        .await;
        assert!(ok.is_ok(), "loosened doing rule must pass without workdir");

        // Drop the rule — the const default re-applies.
        delete(State(state.clone()), Path("doing".into()))
            .await
            .unwrap();

        let err = crate::routes::board::tests_helpers_create(
            state,
            NewBoardItem {
                title: "second one".into(),
                body: None,
                status: Some("doing".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            },
        )
        .await
        .expect_err("const default must reject again");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "doing");
        assert_eq!(
            body["missing"],
            serde_json::json!(["workdir", "tool", "claimed_by"])
        );
    }

    #[tokio::test]
    async fn custom_column_gate_via_rule() {
        // Custom `review` column: PUT a rule requiring
        // session_id_or_comment; PATCH a card into `review` without
        // either → 400. DELETE the rule → same PATCH passes (custom-
        // column passthrough restored).
        let state = fresh_state().await;
        upsert(
            State(state.clone()),
            Path("review".into()),
            Json(UpsertBody {
                required_fields: vec!["title".into(), "lbl".into(), "session_id_or_comment".into()],
            }),
        )
        .await
        .unwrap();

        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "needs review".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        let err = crate::routes::board::tests_helpers_patch(
            state.clone(),
            seed.id,
            BoardPatch {
                status: Some("review".into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("review gate must reject without session_id_or_comment");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "review");
        assert_eq!(
            body["missing"],
            serde_json::json!(["session_id_or_comment"])
        );

        // DELETE the rule — passthrough restored.
        delete(State(state.clone()), Path("review".into()))
            .await
            .unwrap();

        let out = crate::routes::board::tests_helpers_patch(
            state,
            seed.id,
            BoardPatch {
                status: Some("review".into()),
                ..Default::default()
            },
        )
        .await
        .expect("after DELETE, review must passthrough");
        assert_eq!(out.0.status, "review");
    }

    #[tokio::test]
    async fn noop_put_does_not_emit_event_on_second_call() {
        // The first PUT writes the rule and emits `board.rules.updated`.
        // The second PUT with the identical body skips the write and
        // must NOT emit a second event. Listeners care about state
        // transitions, not write attempts.
        let state = fresh_state().await;
        let mut rx = state.bus.subscribe();

        let body = || UpsertBody {
            required_fields: vec!["title".into(), "lbl".into()],
        };

        upsert(State(state.clone()), Path("doing".into()), Json(body()))
            .await
            .unwrap();
        // First event must fire.
        let ev = rx.recv().await.expect("first PUT emits");
        assert_eq!(ev.kind, "board.rules.updated");

        // Second PUT with identical body — no event.
        upsert(State(state.clone()), Path("doing".into()), Json(body()))
            .await
            .unwrap();
        let timed = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(timed.is_err(), "noop PUT must not emit a second event");
    }
}
