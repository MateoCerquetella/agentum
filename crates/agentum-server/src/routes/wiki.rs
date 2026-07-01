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

use std::path::{Path, PathBuf};
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
use crate::wiki::{
    WikiPageMeta, build_wiki_prompt, is_valid_slug, parse_wiki_index, wiki_dir, wiki_key,
    wiki_store_dir,
};

/// Let the generation agent boot, write the wiki, and go idle. `wait_for_settle`
/// returns on the first idle event after `GRACE`, or at `TIMEOUT` regardless.
const WIKI_SETTLE_GRACE: Duration = Duration::from_secs(10);
const WIKI_SETTLE_TIMEOUT: Duration = Duration::from_secs(1200);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/wiki", get(get_index))
        .route("/api/wiki/generate", post(generate))
        // Static segments — matchit prioritizes them over the `{slug}` param route.
        .route("/api/wiki/reindex", post(reindex))
        .route("/api/wiki/export", post(export_to_repo))
        .route("/api/wiki/{slug}", get(get_page))
}

/// Wiki routes are keyed by `repoId` (not a raw path) so the server can resolve
/// the repo's HOST and read its git remote — the identity the central store is
/// keyed on. That's what lets a local checkout and an SSH checkout of the same
/// repo share one wiki.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoQuery {
    repo_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateRequest {
    repo_id: String,
    /// Which agent CLI writes the wiki (default `claude`) — mirrors the Chat
    /// planner's tool pick.
    #[serde(default)]
    tool: Option<String>,
    /// Model hint passed through to the agent's `--model` (e.g. `claude-opus-4-8`);
    /// `None` = the agent's own default.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRequest {
    repo_id: String,
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

/// Everything a wiki route needs for a repo: the git-keyed central store dir (so
/// every checkout of one repo shares it), the checkout path (for legacy migration
/// + save-to-repo), and whether it's remote (SSH). Browse works for remote repos
/// too — host-aware git resolves their identity — but generation stays local.
struct WikiTarget {
    dir: PathBuf,
    path: String,
    is_remote: bool,
}

/// Resolve a repo id → its central wiki store dir, keyed by the repo's git remote
/// (host-aware, so an SSH repo resolves its identity over the connection) with a
/// path fallback when there's no remote.
async fn resolve_target(state: &AppState, repo_id: &str) -> Result<WikiTarget, ApiError> {
    let path = crate::routes::repos::resolve_repo_path(repo_id)?;
    let host = crate::routes::repos::load_host_for_repo(state, repo_id).await?;
    let is_remote = host.id != LOCAL_HOST_ID;
    let remote = crate::host_runtime::git_in_dir(&host, &path, &["remote", "get-url", "origin"])
        .await
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout_string().trim().to_string())
        .filter(|s| !s.is_empty());
    let dir = wiki_store_dir(&wiki_key(remote.as_deref(), &path))
        .map_err(|e| ApiError::Internal(format!("resolve wiki dir: {e}")))?;
    Ok(WikiTarget {
        dir,
        path,
        is_remote,
    })
}

/// One-time migration: if the central store has no wiki yet but a legacy in-repo
/// wiki (`<path>/.agentum/wiki`) exists, copy it into the central store so an
/// already-generated wiki keeps working after the move. Local repos only (we read
/// the legacy dir off the local fs); best-effort — a failure just means the user
/// regenerates.
async fn migrate_legacy_if_needed(target: &WikiTarget) {
    if target.is_remote {
        return;
    }
    if tokio::fs::metadata(target.dir.join("index.json"))
        .await
        .is_ok()
    {
        return; // central store already populated
    }
    let legacy = wiki_dir(Path::new(&target.path));
    if tokio::fs::metadata(legacy.join("index.json"))
        .await
        .is_err()
    {
        return; // nothing to migrate
    }
    let _ = copy_flat_dir(&legacy, &target.dir).await;
}

async fn get_index(
    State(state): State<AppState>,
    Query(q): Query<RepoQuery>,
) -> Result<Json<WikiIndexResponse>, ApiError> {
    let target = resolve_target(&state, &q.repo_id).await?;
    migrate_legacy_if_needed(&target).await;
    Ok(Json(load_index_response(&target.dir).await))
}

async fn get_page(
    State(state): State<AppState>,
    AxumPath(slug): AxumPath<String>,
    Query(q): Query<RepoQuery>,
) -> Result<Json<PageContent>, ApiError> {
    let target = resolve_target(&state, &q.repo_id).await?;
    let content = load_page(&target.dir, &slug).await?;
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
async fn reindex(
    State(state): State<AppState>,
    Query(q): Query<RepoQuery>,
) -> Result<Json<ReindexResponse>, ApiError> {
    let dir = resolve_target(&state, &q.repo_id).await?.dir;
    if tokio::fs::metadata(dir.join("index.json")).await.is_err() {
        return Err(ApiError::BadRequest(
            "no wiki to index for this project — generate the wiki first".into(),
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
    let target = resolve_target(&state, &req.repo_id).await?;
    // Generation runs a LOCAL agent that reads the checkout — a remote/SSH repo
    // has no local checkout to read (its browse still shows a wiki a local sibling
    // of the same git repo generated).
    if target.is_remote {
        return Err(ApiError::BadRequest(
            "wiki generation runs on a local agent — not available for remote/SSH projects yet"
                .into(),
        ));
    }
    let workdir = expand_workdir(&target.path)?;
    if !workdir.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            target.path
        )));
    }
    let dir = target.dir.clone();

    // Full-replace: clear any prior wiki so stale pages can't linger, then recreate
    // the central store dir. No `.gitignore` step — the wiki lives OUTSIDE the repo
    // now; the opt-in "save to repo" export is what puts a committable copy back.
    tokio::fs::remove_dir_all(&dir).await.ok();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::Internal(format!("create wiki dir: {e}")))?;

    // Which agent + model writes the wiki (Chat-style pick). Defaults preserve the
    // prior behaviour: Claude on its own default model.
    let tool = req
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("claude")
        .to_string();
    let model = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    // Build the agent session through the one launch path (YOLO mandatory).
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    let new = NewSession {
        name: format!("autowiki-{}", now_millis()),
        workdir: workdir.to_string_lossy().into_owned(),
        tool,
        model,
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
    let tmux_target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(&state, &session, &host, &tmux_target, &workdir)
        .await?;
    write_status(&dir, "running", sid, None).await;

    // Ground the prompt with the repo-context seed; the agent reads on disk for
    // more. It READS `wd_str` and WRITES the wiki into `out_dir` (the central store).
    let wd_str = workdir.to_string_lossy().into_owned();
    let out_dir = dir.to_string_lossy().into_owned();
    let repo_context = crate::routes::chat::gather_repo_context(Some(&wd_str));
    let prompt = build_wiki_prompt(&wd_str, &out_dir, repo_context.as_deref());

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResponse {
    /// Where the committable copy was written (`<repo>/.agentum/wiki`).
    path: String,
    files: usize,
}

/// `POST /api/wiki/export` — copy the central wiki INTO the repo
/// (`<repo>/.agentum/wiki`) as a committable copy, and make sure `.agentum/wiki`
/// is NOT gitignored, so the user can `git add` + commit it if they want the wiki
/// versioned with the code. Local repos only. Skips the regenerable sidecars
/// (`.status.json`, `.embeddings.json`) — those shouldn't be committed.
async fn export_to_repo(
    State(state): State<AppState>,
    Json(req): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, ApiError> {
    let target = resolve_target(&state, &req.repo_id).await?;
    if target.is_remote {
        return Err(ApiError::BadRequest(
            "save to repo runs locally — not available for remote/SSH projects yet".into(),
        ));
    }
    if tokio::fs::metadata(target.dir.join("index.json"))
        .await
        .is_err()
    {
        return Err(ApiError::BadRequest(
            "no wiki to save — generate the wiki first".into(),
        ));
    }
    let repo_wiki = wiki_dir(Path::new(&target.path));
    let files = copy_wiki_pages(&target.dir, &repo_wiki)
        .await
        .map_err(|e| ApiError::Internal(format!("save to repo: {e}")))?;
    unignore_wiki(Path::new(&target.path)).await;
    Ok(Json(ExportResponse {
        path: repo_wiki.to_string_lossy().into_owned(),
        files,
    }))
}

/// Copy the files (not subdirs — the wiki dir is flat) of `from` into `to`. Used
/// by the legacy migration, which wants every file including the sidecars.
async fn copy_flat_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    tokio::fs::create_dir_all(to).await?;
    let mut rd = tokio::fs::read_dir(from).await?;
    let mut n = 0usize;
    while let Some(entry) = rd.next_entry().await? {
        if entry.file_type().await?.is_file() {
            tokio::fs::copy(entry.path(), to.join(entry.file_name())).await?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copy only the versionable wiki files (`index.json` + `*.md`) from the central
/// store into the repo dir; skip dotfile sidecars so a `git add` stays clean.
async fn copy_wiki_pages(from: &Path, to: &Path) -> std::io::Result<usize> {
    tokio::fs::create_dir_all(to).await?;
    let mut rd = tokio::fs::read_dir(from).await?;
    let mut n = 0usize;
    while let Some(entry) = rd.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let versionable = name == "index.json" || (name.ends_with(".md") && !name.starts_with('.'));
        if versionable {
            tokio::fs::copy(entry.path(), to.join(name.as_ref())).await?;
            n += 1;
        }
    }
    Ok(n)
}

/// Remove a `wiki/` ignore line from `<repo>/.agentum/.gitignore` so an exported
/// wiki is committable (older builds gitignored it). Best-effort; no-op if the
/// file or the line is absent.
async fn unignore_wiki(workdir: &Path) {
    let gitignore = workdir.join(".agentum").join(".gitignore");
    let Ok(existing) = tokio::fs::read_to_string(&gitignore).await else {
        return;
    };
    if !existing.lines().any(|l| l.trim() == "wiki/") {
        return;
    }
    let mut out = existing
        .lines()
        .filter(|l| l.trim() != "wiki/")
        .collect::<Vec<_>>()
        .join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let _ = tokio::fs::write(&gitignore, out).await;
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

    #[tokio::test]
    async fn export_copies_pages_and_skips_sidecars() {
        // Save-to-repo must copy the versionable files and leave the regenerable
        // sidecars behind (they shouldn't land in git).
        let src = temp_dir();
        tokio::fs::write(src.join("index.json"), "{}")
            .await
            .unwrap();
        tokio::fs::write(src.join("overview.md"), "# o")
            .await
            .unwrap();
        tokio::fs::write(src.join("architecture.md"), "# a")
            .await
            .unwrap();
        tokio::fs::write(src.join(".status.json"), "{}")
            .await
            .unwrap();
        tokio::fs::write(src.join(".embeddings.json"), "{}")
            .await
            .unwrap();
        let dst = temp_dir();
        let n = copy_wiki_pages(&src, &dst).await.unwrap();
        assert_eq!(n, 3); // index.json + 2 pages, no sidecars
        assert!(tokio::fs::metadata(dst.join("index.json")).await.is_ok());
        assert!(tokio::fs::metadata(dst.join("overview.md")).await.is_ok());
        assert!(tokio::fs::metadata(dst.join(".status.json")).await.is_err());
        assert!(
            tokio::fs::metadata(dst.join(".embeddings.json"))
                .await
                .is_err()
        );
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[tokio::test]
    async fn unignore_wiki_drops_the_wiki_line_and_keeps_the_rest() {
        let repo = temp_dir();
        let agentum = repo.join(".agentum");
        tokio::fs::create_dir_all(&agentum).await.unwrap();
        tokio::fs::write(agentum.join(".gitignore"), "foo\nwiki/\nbar\n")
            .await
            .unwrap();
        unignore_wiki(&repo).await;
        let after = tokio::fs::read_to_string(agentum.join(".gitignore"))
            .await
            .unwrap();
        assert!(!after.lines().any(|l| l.trim() == "wiki/"));
        assert!(after.contains("foo"));
        assert!(after.contains("bar"));
        // No .gitignore at all → no-op, no panic.
        let bare = temp_dir();
        unignore_wiki(&bare).await;
        std::fs::remove_dir_all(&repo).ok();
        std::fs::remove_dir_all(&bare).ok();
    }
}
