//! `/api/wiki` — AutoWiki generation + browse (spec 001).
//!
//! - `GET  /api/wiki?workdir=`        → the TOC: Empty | Running | Ready | Failed
//! - `GET  /api/wiki/{slug}?workdir=` → one page's markdown
//! - `POST /api/wiki/generate`        → spawn an agent that WRITES the wiki; returns its session id
//!
//! `generate` transposes the harness QA-gate recipe
//! (`harness::drive::run_qa_agent_gate`): spawn through the *one* launch path →
//! inject the prompt → wait for the agent to settle → tear it down → read
//! `index.json` back. A missing/garbled index is a **failure** (AC-9), recorded in
//! `.status.json` so the browse view shows an error, never a half-empty success.
//! It returns the `session_id` immediately (job model) so the desktop can stream
//! the pane live and re-fetch `GET /api/wiki` on `agent.finished`.

use std::path::Path;
use std::time::Duration;

use agentum_core::{LOCAL_HOST_ID, NewSession};
use axum::Json;
use axum::Router;
use axum::extract::{Path as AxumPath, Query, State};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::routes::util::{expand_workdir, now_millis};
use crate::wiki::{WikiPageMeta, build_wiki_prompt, is_valid_slug, parse_wiki_index, wiki_dir};

/// Let the generation agent boot, write the wiki, and go idle. `wait_for_settle`
/// returns on the first idle event after `GRACE`, or at `TIMEOUT` regardless.
const WIKI_SETTLE_GRACE: Duration = Duration::from_secs(10);
const WIKI_SETTLE_TIMEOUT: Duration = Duration::from_secs(1200);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/wiki", get(get_index))
        .route("/api/wiki/generate", post(generate))
        // Static segment — matchit prioritizes it over the `{slug}` param route.
        .route("/api/wiki/reindex", post(reindex))
        .route("/api/wiki/{slug}", get(get_page))
}

#[derive(Deserialize)]
struct WorkdirQuery {
    workdir: String,
}

#[derive(Deserialize)]
struct GenerateRequest {
    workdir: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateResponse {
    session_id: Uuid,
}

#[derive(Serialize)]
struct PageContent {
    content: String,
}

/// The browse-time state of a workdir's wiki. Internally tagged on `state` so the
/// desktop switches on one discriminator.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum WikiIndexResponse {
    /// Never generated.
    Empty,
    /// A generation run is in flight; stream `session_id`'s pane to watch it.
    Running { session_id: Uuid },
    /// The last run failed (or wrote no/garbled index) — never a half-empty wiki.
    Failed { error: String },
    /// A wiki is present.
    Ready {
        schema_version: u32,
        generated_at: u64,
        pages: Vec<WikiPageMeta>,
    },
}

/// The run-status sidecar the generate task writes (`.agentum/wiki/.status.json`),
/// read back by `GET /api/wiki` to tell Running from Failed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WikiStatus {
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn get_index(Query(q): Query<WorkdirQuery>) -> Result<Json<WikiIndexResponse>, ApiError> {
    let dir = wiki_dir(&expand_workdir(&q.workdir)?);
    Ok(Json(load_index_response(&dir).await))
}

async fn get_page(
    AxumPath(slug): AxumPath<String>,
    Query(q): Query<WorkdirQuery>,
) -> Result<Json<PageContent>, ApiError> {
    let dir = wiki_dir(&expand_workdir(&q.workdir)?);
    let content = load_page(&dir, &slug).await?;
    Ok(Json(PageContent { content }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReindexResponse {
    /// Number of chunks embedded into the sidecar.
    chunks: usize,
}

/// `POST /api/wiki/reindex?workdir=` — (re)build the RAG embedding sidecar
/// (spec 003) for an already-generated wiki, without re-running the generation
/// agent. Lets an existing wiki gain retrieval, or refresh after a model change.
async fn reindex(Query(q): Query<WorkdirQuery>) -> Result<Json<ReindexResponse>, ApiError> {
    let dir = wiki_dir(&expand_workdir(&q.workdir)?);
    if tokio::fs::metadata(dir.join("index.json")).await.is_err() {
        return Err(ApiError::BadRequest(
            "no wiki to index for this workdir — generate the wiki first".into(),
        ));
    }
    // Blocking fs + embedding math → off the async runtime.
    let chunks = tokio::task::spawn_blocking(move || {
        let embedder = crate::wiki_rag::default_embedder();
        let index = crate::wiki_rag::build_index(&dir, embedder.as_ref())?;
        crate::wiki_rag::save_index(&dir, &index)?;
        anyhow::Ok(index.chunks.len())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("reindex task panicked: {e}")))?
    .map_err(|e| ApiError::Internal(format!("reindex failed: {e}")))?;
    Ok(Json(ReindexResponse { chunks }))
}

/// Build + persist the wiki RAG embedding sidecar (spec 003), off the runtime.
/// Best-effort: any failure is swallowed — the wiki works without RAG; retrieval
/// simply finds no sidecar and falls back to no wiki context.
async fn build_embeddings_sidecar(dir: &Path) {
    let dir = dir.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        let embedder = crate::wiki_rag::default_embedder();
        if let Ok(index) = crate::wiki_rag::build_index(&dir, embedder.as_ref()) {
            let _ = crate::wiki_rag::save_index(&dir, &index);
        }
    })
    .await;
}

async fn generate(
    State(state): State<AppState>,
    Json(req): Json<GenerateRequest>,
) -> Result<Json<GenerateResponse>, ApiError> {
    let workdir = expand_workdir(&req.workdir)?;
    if !workdir.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            req.workdir
        )));
    }
    let dir = wiki_dir(&workdir);

    // Full-replace: clear any prior wiki so stale pages can't linger, then recreate.
    tokio::fs::remove_dir_all(&dir).await.ok();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::Internal(format!("create wiki dir: {e}")))?;

    // Keep the wiki app-local by default via a self-contained `.agentum/.gitignore`
    // (never touch the user's tracked root `.gitignore`).
    let gitignore = workdir.join(".agentum").join(".gitignore");
    let existing = tokio::fs::read_to_string(&gitignore)
        .await
        .unwrap_or_default();
    if let Some(updated) = gitignore_ensuring_wiki(&existing) {
        tokio::fs::write(&gitignore, updated).await.ok();
    }

    // Build the agent session through the one launch path (YOLO mandatory).
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    let new = NewSession {
        name: format!("autowiki-{}", now_millis()),
        workdir: workdir.to_string_lossy().into_owned(),
        tool: "claude".to_string(),
        model: None,
        flags: vec![agentum_executor::YOLO_MARKER.to_string()],
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    let session = state
        .store
        .create_session_on_host(new, Some(LOCAL_HOST_ID))
        .await?;
    let sid = session.id;
    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(&state, &session, &host, &target, &workdir)
        .await?;
    write_status(&dir, "running", sid, None).await;

    // Ground the prompt with the repo-context seed; the agent reads on disk for more.
    let wd_str = workdir.to_string_lossy().into_owned();
    let repo_context = crate::routes::chat::gather_repo_context(Some(&wd_str));
    let prompt = build_wiki_prompt(&wd_str, repo_context.as_deref());

    // Drive to completion off-request: inject → settle → teardown → read back.
    let st = state.clone();
    let dir_bg = dir.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::harness::inject_prompt(&st, &session, &prompt).await {
            write_status(
                &dir_bg,
                "failed",
                sid,
                Some(format!("failed to start the wiki agent: {e}")),
            )
            .await;
            crate::harness::teardown_session(&st, &session).await;
            return;
        }
        crate::harness::wait_for_settle(
            &st.bus,
            session.id,
            WIKI_SETTLE_GRACE,
            WIKI_SETTLE_TIMEOUT,
        )
        .await;
        crate::harness::teardown_session(&st, &session).await;

        // AC-9: a valid index ⇒ success (drop the sidecar so GET reports Ready);
        // a missing/garbled index ⇒ failure recorded for the browse view.
        match tokio::fs::read_to_string(dir_bg.join("index.json")).await {
            Ok(raw) => match parse_wiki_index(&raw) {
                Ok(_) => {
                    tokio::fs::remove_file(dir_bg.join(".status.json"))
                        .await
                        .ok();
                    // Build the RAG embedding sidecar (spec 003) so Chat can
                    // retrieve from this wiki. Best-effort: the wiki is fully
                    // usable without it; a failure just means no RAG grounding.
                    build_embeddings_sidecar(&dir_bg).await;
                }
                Err(e) => {
                    write_status(
                        &dir_bg,
                        "failed",
                        sid,
                        Some(format!("the wiki agent wrote an invalid index.json: {e}")),
                    )
                    .await;
                }
            },
            Err(_) => {
                write_status(
                    &dir_bg,
                    "failed",
                    sid,
                    Some("the wiki agent wrote no index.json (inconclusive)".into()),
                )
                .await;
            }
        }
    });

    Ok(Json(GenerateResponse { session_id: sid }))
}

// ---- testable helpers (no AppState) -----------------------------------------

async fn load_index_response(dir: &Path) -> WikiIndexResponse {
    let index_path = dir.join("index.json");
    if let Ok(raw) = tokio::fs::read_to_string(&index_path).await {
        if let Ok(index) = parse_wiki_index(&raw) {
            // `Ready` must mean *browseable*: the index parses AND every listed
            // page's `<slug>.md` is on disk. The generation agent writes
            // index.json a beat before the page files, so trusting the index
            // alone flips the view to `ready` mid-run and 404s the first page
            // fetch (`GET /api/wiki/<slug>`) until the file lands. While a page
            // is still missing we fall through to the run sidecar (Running),
            // so the UI keeps polling instead of requesting a not-yet-written
            // page. All pages present ⇒ Ready even if a stale `running` sidecar
            // lingers (a mid-generation app restart orphans it).
            if all_pages_present(dir, &index.pages).await {
                return WikiIndexResponse::Ready {
                    schema_version: index.schema_version,
                    generated_at: file_mtime_millis(&index_path).await,
                    pages: index.pages,
                };
            }
        }
    }
    if let Ok(raw) = tokio::fs::read_to_string(dir.join(".status.json")).await {
        if let Ok(st) = serde_json::from_str::<WikiStatus>(&raw) {
            return match st.state.as_str() {
                "running" => WikiIndexResponse::Running {
                    session_id: st.session_id.unwrap_or_default(),
                },
                _ => WikiIndexResponse::Failed {
                    error: st.error.unwrap_or_else(|| "wiki generation failed".into()),
                },
            };
        }
    }
    WikiIndexResponse::Empty
}

/// True iff every listed page's `<slug>.md` exists in `dir`. Gates `Ready` so a
/// partially-written wiki (index.json present, page files still landing) never
/// shows as browseable — the source of the transient `GET /api/wiki/<slug>` 404.
/// Slugs come from `parse_wiki_index`, which already rejects path-traversal.
async fn all_pages_present(dir: &Path, pages: &[WikiPageMeta]) -> bool {
    for page in pages {
        if tokio::fs::metadata(dir.join(format!("{}.md", page.slug)))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

async fn load_page(dir: &Path, slug: &str) -> Result<String, ApiError> {
    if !is_valid_slug(slug) {
        return Err(ApiError::BadRequest(format!("invalid wiki slug: {slug}")));
    }
    tokio::fs::read_to_string(dir.join(format!("{slug}.md")))
        .await
        .map_err(|_| ApiError::NotFound(format!("wiki page: {slug}")))
}

async fn write_status(dir: &Path, state: &str, session_id: Uuid, error: Option<String>) {
    let status = WikiStatus {
        state: state.to_string(),
        session_id: Some(session_id),
        error,
    };
    if let Ok(json) = serde_json::to_string_pretty(&status) {
        let _ = tokio::fs::write(dir.join(".status.json"), json).await;
    }
}

async fn file_mtime_millis(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The new `.agentum/.gitignore` content that ensures `wiki/` is ignored, or
/// `None` if it already is. Pure for testability.
fn gitignore_ensuring_wiki(existing: &str) -> Option<String> {
    if existing.lines().any(|l| l.trim() == "wiki/") {
        return None;
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("wiki/\n");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("autowiki-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn index_empty_when_nothing_generated() {
        let d = temp_dir();
        assert!(matches!(
            load_index_response(&d).await,
            WikiIndexResponse::Empty
        ));
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn index_ready_round_trips_a_fixture() {
        let d = temp_dir();
        tokio::fs::write(
            d.join("index.json"),
            r#"{"schemaVersion":1,"pages":[{"slug":"overview","title":"Overview"},{"slug":"architecture","title":"Architecture"}]}"#,
        )
        .await
        .unwrap();
        // Ready requires the page files too — a complete wiki has both on disk.
        tokio::fs::write(d.join("overview.md"), "# Overview")
            .await
            .unwrap();
        tokio::fs::write(d.join("architecture.md"), "# Architecture")
            .await
            .unwrap();
        match load_index_response(&d).await {
            WikiIndexResponse::Ready {
                schema_version,
                pages,
                ..
            } => {
                assert_eq!(schema_version, 1);
                assert_eq!(pages.len(), 2);
                assert_eq!(pages[0].slug, "overview");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn index_running_while_pages_still_writing() {
        // The race: the agent has written index.json + a `running` sidecar, but
        // the page files haven't all landed yet. Must NOT report Ready (which
        // would make the UI request a not-yet-written page and 404) — report
        // Running so the UI keeps polling.
        let d = temp_dir();
        tokio::fs::write(
            d.join("index.json"),
            r#"{"schemaVersion":1,"pages":[{"slug":"overview","title":"Overview"},{"slug":"architecture","title":"Architecture"}]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            d.join(".status.json"),
            r#"{"state":"running","sessionId":"00000000-0000-0000-0000-000000000000"}"#,
        )
        .await
        .unwrap();
        // Only the first page exists so far.
        tokio::fs::write(d.join("overview.md"), "# Overview")
            .await
            .unwrap();
        assert!(matches!(
            load_index_response(&d).await,
            WikiIndexResponse::Running { .. }
        ));
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn index_ready_ignores_stale_running_sidecar() {
        // A complete wiki (index + all pages) must resolve to Ready even if a
        // `running` sidecar was orphaned by a mid-generation app restart.
        let d = temp_dir();
        tokio::fs::write(
            d.join("index.json"),
            r#"{"schemaVersion":1,"pages":[{"slug":"overview","title":"Overview"}]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(d.join("overview.md"), "# Overview")
            .await
            .unwrap();
        tokio::fs::write(
            d.join(".status.json"),
            r#"{"state":"running","sessionId":"00000000-0000-0000-0000-000000000000"}"#,
        )
        .await
        .unwrap();
        assert!(matches!(
            load_index_response(&d).await,
            WikiIndexResponse::Ready { .. }
        ));
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn index_running_then_failed_from_status_sidecar() {
        let d = temp_dir();
        tokio::fs::write(
            d.join(".status.json"),
            r#"{"state":"running","sessionId":"00000000-0000-0000-0000-000000000000"}"#,
        )
        .await
        .unwrap();
        assert!(matches!(
            load_index_response(&d).await,
            WikiIndexResponse::Running { .. }
        ));
        tokio::fs::write(
            d.join(".status.json"),
            r#"{"state":"failed","error":"boom"}"#,
        )
        .await
        .unwrap();
        match load_index_response(&d).await {
            WikiIndexResponse::Failed { error } => assert_eq!(error, "boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn page_rejects_traversal_then_404_then_reads() {
        let d = temp_dir();
        assert!(matches!(
            load_page(&d, "../etc/passwd").await,
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            load_page(&d, "missing").await,
            Err(ApiError::NotFound(_))
        ));
        tokio::fs::write(d.join("overview.md"), "# hi")
            .await
            .unwrap();
        assert_eq!(load_page(&d, "overview").await.unwrap(), "# hi");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn gitignore_adds_wiki_once() {
        assert_eq!(gitignore_ensuring_wiki("").as_deref(), Some("wiki/\n"));
        assert_eq!(
            gitignore_ensuring_wiki("foo\n").as_deref(),
            Some("foo\nwiki/\n")
        );
        assert_eq!(
            gitignore_ensuring_wiki("bar").as_deref(),
            Some("bar\nwiki/\n")
        );
        assert_eq!(gitignore_ensuring_wiki("wiki/\n"), None);
        assert_eq!(gitignore_ensuring_wiki("a\nwiki/\nb\n"), None);
    }
}
