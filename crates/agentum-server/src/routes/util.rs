//! Shared route helpers.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agentum_core::{Host, LOCAL_HOST_ID};
use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use super::board_goals::SlugReason;
use crate::AppState;
use crate::error::ApiError;

/// Expand `~` / `~/x` to the daemon's `$HOME` and trim trailing
/// slashes (preserving a bare `/`). Other paths pass through unchanged.
///
/// The dashboard's `DirPicker` placeholder hints at `~/projects/foo`,
/// and users typing or pasting tilde-prefixed paths used to hit a
/// `400 workdir does not exist` because `PathBuf::from("~/…").exists()`
/// is always false — tilde expansion is a shell concern, not an OS one.
/// `/api/fs/list` already resolves the same way, so every workdir
/// gate now matches the picker's behaviour.
pub(crate) fn expand_workdir(raw: &str) -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    expand_with_home(raw, home.as_deref())
}

fn expand_with_home(raw: &str, home: Option<&Path>) -> Result<PathBuf, ApiError> {
    let trimmed = raw.trim();
    let trimmed = if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("workdir is empty".into()));
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        let home = home.ok_or_else(|| ApiError::Internal("HOME not set".into()))?;
        if trimmed == "~" {
            return Ok(home.to_path_buf());
        }
        return Ok(home.join(&trimmed[2..]));
    }
    Ok(PathBuf::from(trimmed))
}

/// Parse a path-segment UUID, mapping a malformed id to a 400. Shared by the
/// session/git/host/upload routes (previously copy-pasted into each).
pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// Host a `{workdir, slug?, repoId?}` tracker request reads git state on
/// (spec 020 D1): an explicit repoId wins — an unknown id is a 4xx, NEVER a
/// silent local fallback (an identity error is a client bug that must be
/// loud); an absent (or blank) repoId is the local host, i.e. today's
/// behavior for every existing caller.
pub(crate) async fn resolve_tracker_host(
    state: &AppState,
    repo_id: Option<&str>,
) -> Result<Host, ApiError> {
    match repo_id.map(str::trim).filter(|s| !s.is_empty()) {
        // 404 unknown id / 400 deleted host — both from load_host_for_repo.
        Some(id) => super::repos::load_host_for_repo(state, id).await,
        None => state
            .store
            .get_host(LOCAL_HOST_ID)
            .await?
            .ok_or_else(|| ApiError::Internal("local host missing".into())),
    }
}

/// The ONE `{workdir, slug?, repoId?}` → slug resolver with the typed
/// `no_github_repo` 422 (spec 020 F1: unifies the admitted duplicates that
/// lived in routes::github_projects and routes::provision). Order matters:
/// workdir shape-check → expand → host (repoId-aware) → `resolve_github_slug`
/// (whose hint fast-path never touches git). The host resolves BEFORE the
/// hint short-circuit so a garbage repoId 4xxes even when a valid hint would
/// have answered — honoring the hint would mask the identity error. A valid
/// hint still performs zero git I/O: the repoId branch reads the JSON
/// registry + the host row only.
pub(crate) async fn resolve_tracker_slug(
    state: &AppState,
    repo_id: Option<&str>,
    workdir: &str,
    slug_hint: Option<&str>,
) -> Result<String, ApiError> {
    let workdir = workdir.trim();
    if workdir.is_empty() {
        return Err(ApiError::BadRequest("`workdir` is required".into()));
    }
    // Expand `~`/trailing-slash before the git read. `resolve_github_slug`
    // runs `git -C <workdir>` with no shell, so a stored `~/…` project path is
    // passed literally and git can't cd into it — the origin read fails and
    // the caller dead-ends on a spurious `no_github_repo`. No-op for the
    // common absolute path. (Expansion is against the daemon's HOME — wrong
    // in principle for a remote repoId, but remote registry paths are
    // absolute by construction, so it stays a no-op there; the pre-existing
    // edge every `base_ref_*` route shares.)
    let expanded = expand_workdir(workdir)?;
    let workdir = expanded.to_string_lossy();
    let host = resolve_tracker_host(state, repo_id).await?;
    super::board_goals::resolve_github_slug(&host, &workdir, slug_hint)
        .await
        .map_err(|reason| {
            let (status, body) = no_github_repo_envelope(reason);
            ApiError::Custom(status, body)
        })
}

/// Pure: the typed 422 body for a slug miss. Keeps the `no_github_repo` code
/// the UI branches on, but carries the real reason in the message — a repo
/// whose `origin` isn't a GitHub remote must not be indistinguishable from an
/// unreachable host (spec 020; the github_projects.rs precedent).
pub(crate) fn no_github_repo_envelope(reason: SlugReason) -> (StatusCode, serde_json::Value) {
    let message = match reason {
        SlugReason::NoGithubRemote => {
            "no GitHub repo resolved for this project — its folder has no `origin` remote pointing at GitHub"
        }
        SlugReason::HostUnreachable => {
            "no GitHub repo resolved for this project — could not read the repo's git origin"
        }
    };
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "error": { "code": "no_github_repo", "message": message } }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::LOCAL_HOST_ID;
    use agentum_store::Store;
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    /// In-process AppState harness — the board_goals.rs tests' `fresh_state`
    /// (the store seeds the local host row on open, which is what the
    /// absent-repoId arm reads).
    async fn fresh_state() -> crate::AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        crate::AppState {
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
            wiki_keys: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
            sdd_loops: Default::default(),
            events_ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    // ── spec 020 F1: host-aware slug resolution ─────────────────────────────

    /// Absent (or blank) repoId = the local host — today's behavior for every
    /// existing caller, byte-for-byte (spec 020 D1's regression half).
    #[tokio::test]
    async fn resolve_tracker_host_absent_repo_id_is_local() {
        let state = fresh_state().await;
        let host = resolve_tracker_host(&state, None).await.unwrap();
        assert_eq!(host.id, LOCAL_HOST_ID);
        // A blank id is "absent", not an identity claim — the trim/filter arm.
        let host = resolve_tracker_host(&state, Some("  ")).await.unwrap();
        assert_eq!(host.id, LOCAL_HOST_ID);
    }

    /// Unknown repoId is a loud 4xx, never a silent local fallback (D1).
    /// Env-tolerant: a random uuid misses whatever `~/.agentum/repos.json`
    /// holds, so no env mutation is needed (the 015 house rule).
    #[tokio::test]
    async fn resolve_tracker_host_unknown_repo_id_is_4xx() {
        let state = fresh_state().await;
        let id = format!("020-no-such-repo-{}", Uuid::new_v4());
        let err = resolve_tracker_host(&state, Some(&id)).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    }

    /// AC 2: a valid `owner/repo` hint performs ZERO git I/O — proven by the
    /// unreadable workdir succeeding. (The resolver-level twin lives in
    /// board_goals tests; this one pins the route-family path including the
    /// host-load ordering.)
    #[tokio::test]
    async fn resolve_tracker_slug_hint_short_circuits_with_unreadable_workdir() {
        let state = fresh_state().await;
        let slug = resolve_tracker_slug(&state, None, "/path/does/not/exist", Some("acme/widgets"))
            .await
            .unwrap();
        assert_eq!(slug, "acme/widgets");
    }

    /// The ordering contract: an unknown repoId 4xxes even when a valid hint
    /// would have short-circuited — an identity error is a client bug that
    /// must be loud; honoring the hint would mask it.
    #[tokio::test]
    async fn resolve_tracker_slug_unknown_repo_id_beats_valid_hint() {
        let state = fresh_state().await;
        let id = format!("020-no-such-repo-{}", Uuid::new_v4());
        let err = resolve_tracker_slug(
            &state,
            Some(&id),
            "/path/does/not/exist",
            Some("acme/widgets"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    }

    /// Pure: both slug-miss reasons ride the same 422 `no_github_repo`
    /// envelope (the code the UI branches on), but the messages must stay
    /// distinguishable — an unreachable host is not "no origin".
    #[test]
    fn no_github_repo_envelope_distinguishes_reasons() {
        use super::super::board_goals::SlugReason;
        let (s1, b1) = no_github_repo_envelope(SlugReason::NoGithubRemote);
        let (s2, b2) = no_github_repo_envelope(SlugReason::HostUnreachable);
        assert_eq!(s1, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(s2, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(b1["error"]["code"], "no_github_repo");
        assert_eq!(b2["error"]["code"], "no_github_repo");
        let m1 = b1["error"]["message"].as_str().unwrap();
        let m2 = b2["error"]["message"].as_str().unwrap();
        assert_ne!(m1, m2, "reasons must not collapse into one message");
        assert!(m1.contains("origin"), "{m1}");
        assert!(m2.contains("could not read"), "{m2}");
    }

    #[test]
    fn expands_bare_tilde() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            expand_with_home("~", Some(&home)).unwrap(),
            PathBuf::from("/home/test")
        );
        assert_eq!(
            expand_with_home("~/", Some(&home)).unwrap(),
            PathBuf::from("/home/test")
        );
        assert_eq!(
            expand_with_home("~/projects/foo", Some(&home)).unwrap(),
            PathBuf::from("/home/test/projects/foo")
        );
        assert_eq!(
            expand_with_home("~/projects/foo/", Some(&home)).unwrap(),
            PathBuf::from("/home/test/projects/foo")
        );
    }

    #[test]
    fn absolute_pass_through_with_trailing_slash_trim() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            expand_with_home("/var/log/", Some(&home)).unwrap(),
            PathBuf::from("/var/log")
        );
        assert_eq!(
            expand_with_home("/", Some(&home)).unwrap(),
            PathBuf::from("/")
        );
        assert_eq!(
            expand_with_home("  /tmp  ", Some(&home)).unwrap(),
            PathBuf::from("/tmp")
        );
    }

    #[test]
    fn empty_is_rejected() {
        let home = PathBuf::from("/home/test");
        assert!(matches!(
            expand_with_home("   ", Some(&home)).unwrap_err(),
            ApiError::BadRequest(_)
        ));
    }

    #[test]
    fn tilde_without_home_errors_internal() {
        assert!(matches!(
            expand_with_home("~/foo", None).unwrap_err(),
            ApiError::Internal(_)
        ));
    }

    #[test]
    fn non_tilde_paths_dont_need_home() {
        assert_eq!(
            expand_with_home("/abs/path", None).unwrap(),
            PathBuf::from("/abs/path")
        );
    }
}
