//! Canonical per-project tracker contract. Repo.id is the ownership key; every
//! mutation is revision-checked in SQLite and provider-specific stores are
//! migration/compatibility inputs only.

use agentum_core::{
    PROJECT_TRACKER_SCHEMA_VERSION, ProjectTrackerBoardBinding, ProjectTrackerConfig,
    ProjectTrackerGithubTarget, ProjectTrackerPreferences, ProjectTrackerProvenance,
    ProjectTrackerProvider, ProjectTrackerStatusMapping,
};
use agentum_store::ProjectTrackerWrite;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;
use crate::github_projects::{BoardBinding, StatusMapping, StatusNames};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/repos/{repo_id}/tracker-config",
        get(get_config)
            .put(put_config)
            .patch(patch_preferences)
            .delete(delete_config),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetResponse {
    config: Option<ProjectTrackerConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    migration_conflict: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutRequest {
    expected_revision: Option<i64>,
    config: ProjectTrackerConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreferencesRequest {
    expected_revision: i64,
    preferences: ProjectTrackerPreferences,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteQuery {
    expected_revision: Option<i64>,
}

fn conflict(current: Option<ProjectTrackerConfig>) -> ApiError {
    ApiError::Custom(
        StatusCode::CONFLICT,
        serde_json::json!({ "error": "tracker config revision conflict", "current": current }),
    )
}

fn validate(config: &ProjectTrackerConfig, repo_id: &str) -> Result<(), ApiError> {
    if config.schema_version != PROJECT_TRACKER_SCHEMA_VERSION {
        return Err(ApiError::BadRequest(
            "unsupported tracker schemaVersion".into(),
        ));
    }
    if config.repo_id != repo_id {
        return Err(ApiError::BadRequest(
            "config.repoId must match the route repo id".into(),
        ));
    }
    match config.provider {
        None if config.github.is_some() || config.linear.is_some() => Err(ApiError::BadRequest(
            "unconfigured tracker cannot carry a provider target".into(),
        )),
        Some(ProjectTrackerProvider::Github) => {
            let github = config.github.as_ref().ok_or_else(|| {
                ApiError::BadRequest("github target is required for provider github".into())
            })?;
            if github.repository_slug.trim().split('/').count() != 2 {
                return Err(ApiError::BadRequest(
                    "github.repositorySlug must be owner/repository".into(),
                ));
            }
            if config.linear.is_some() {
                return Err(ApiError::BadRequest(
                    "inactive linear target is not allowed".into(),
                ));
            }
            Ok(())
        }
        Some(ProjectTrackerProvider::Linear) => {
            let linear = config.linear.as_ref().ok_or_else(|| {
                ApiError::BadRequest("linear target is required for provider linear".into())
            })?;
            if linear.workspace_id.trim().is_empty() {
                return Err(ApiError::BadRequest(
                    "linear.workspaceId is required".into(),
                ));
            }
            if config.github.is_some() {
                return Err(ApiError::BadRequest(
                    "inactive github target is not allowed".into(),
                ));
            }
            Ok(())
        }
        None => Ok(()),
    }
}

fn unconfigured(repo_id: &str, provenance: ProjectTrackerProvenance) -> ProjectTrackerConfig {
    ProjectTrackerConfig {
        schema_version: PROJECT_TRACKER_SCHEMA_VERSION,
        repo_id: repo_id.to_string(),
        revision: 0,
        provider: None,
        github: None,
        linear: None,
        task_preferences: Default::default(),
        provenance,
    }
}

fn to_canonical_binding(binding: BoardBinding) -> ProjectTrackerBoardBinding {
    let mapping = |m: StatusMapping| ProjectTrackerStatusMapping {
        todo: m.todo,
        in_progress: m.in_progress,
        in_review: m.in_review,
        ready_to_test: m.ready_to_test,
        done: m.done,
        blocked: m.blocked,
    };
    ProjectTrackerBoardBinding {
        project_id: binding.project_id,
        status_field_id: binding.status_field_id,
        status_mapping: mapping(binding.status_mapping),
        done_closes_issue: binding.done_closes_issue,
        project_title: binding.project_title,
        project_owner: binding.project_owner,
        project_owner_type: binding.project_owner_type,
        project_number: binding.project_number,
        option_names: binding
            .option_names
            .map(|n: StatusNames| ProjectTrackerStatusMapping {
                todo: n.todo,
                in_progress: n.in_progress,
                in_review: n.in_review,
                ready_to_test: n.ready_to_test,
                done: n.done,
                blocked: n.blocked,
            }),
    }
}

pub(crate) fn from_canonical_binding(binding: &ProjectTrackerBoardBinding) -> BoardBinding {
    BoardBinding {
        project_id: binding.project_id.clone(),
        status_field_id: binding.status_field_id.clone(),
        status_mapping: StatusMapping {
            todo: binding.status_mapping.todo.clone(),
            in_progress: binding.status_mapping.in_progress.clone(),
            in_review: binding.status_mapping.in_review.clone(),
            ready_to_test: binding.status_mapping.ready_to_test.clone(),
            done: binding.status_mapping.done.clone(),
            blocked: binding.status_mapping.blocked.clone(),
        },
        done_closes_issue: binding.done_closes_issue,
        project_title: binding.project_title.clone(),
        project_owner: binding.project_owner.clone(),
        project_owner_type: binding.project_owner_type.clone(),
        project_number: binding.project_number,
        option_names: binding.option_names.as_ref().map(|n| StatusNames {
            todo: n.todo.clone(),
            in_progress: n.in_progress.clone(),
            in_review: n.in_review.clone(),
            ready_to_test: n.ready_to_test.clone(),
            done: n.done.clone(),
            blocked: n.blocked.clone(),
        }),
    }
}

async fn migrate(
    state: &AppState,
    repo_id: &str,
) -> Result<(Option<ProjectTrackerConfig>, Option<String>), ApiError> {
    let legacy_provider = super::repos::legacy_tracker_provider(repo_id)?;
    // Resolve on the repo's registered host. Failure is meaningful only when
    // GitHub is the selected/migratable provider; Linear remains unconfigured
    // until the UI submits its repo-keyed context hint.
    let path = super::repos::resolve_repo_path(repo_id)?;
    let slug = super::util::resolve_tracker_slug(state, Some(repo_id), &path, None)
        .await
        .ok();
    let binding = slug
        .as_deref()
        .and_then(crate::github_projects::binding_for_slug);
    if legacy_provider.as_deref() == Some("linear") && binding.is_some() {
        let candidate = unconfigured(repo_id, ProjectTrackerProvenance::Migrated);
        let config = match state
            .store
            .put_project_tracker_config(candidate, None)
            .await?
        {
            ProjectTrackerWrite::Written(config) => Some(config),
            ProjectTrackerWrite::Conflict(current) => current,
        };
        return Ok((
            config,
            Some("legacy Linear pin conflicts with the exact-slug GitHub binding".into()),
        ));
    }
    let provider = if binding.is_some() || legacy_provider.as_deref() == Some("github") {
        Some(ProjectTrackerProvider::Github)
    } else {
        None
    };
    let candidate = match (provider, slug) {
        (Some(provider), Some(slug)) => ProjectTrackerConfig {
            schema_version: PROJECT_TRACKER_SCHEMA_VERSION,
            repo_id: repo_id.to_string(),
            revision: 0,
            provider: Some(provider),
            github: Some(ProjectTrackerGithubTarget {
                repository_slug: slug,
                project_binding: binding.map(to_canonical_binding),
            }),
            linear: None,
            task_preferences: Default::default(),
            provenance: ProjectTrackerProvenance::Migrated,
        },
        _ => unconfigured(repo_id, ProjectTrackerProvenance::Migrated),
    };
    match state
        .store
        .put_project_tracker_config(candidate, None)
        .await?
    {
        ProjectTrackerWrite::Written(config) => Ok((Some(config), None)),
        ProjectTrackerWrite::Conflict(current) => Ok((current, None)),
    }
}

/// Compatibility projection for the old GitHub binding endpoints. Once a
/// repoId is present those endpoints never write the legacy slug-keyed file.
pub(crate) async fn compatibility_get(
    state: &AppState,
    repo_id: &str,
    resolved_slug: &str,
) -> Result<Option<BoardBinding>, ApiError> {
    let mut config = match state.store.get_project_tracker_config(repo_id).await? {
        Some(config) => config,
        None => migrate(state, repo_id)
            .await?
            .0
            .unwrap_or_else(|| unconfigured(repo_id, ProjectTrackerProvenance::Migrated)),
    };
    let Some(github) = config.github.as_ref() else {
        return Ok(None);
    };
    // `Repo.id` is the ownership key, but it is not proof that an old row still
    // describes the repository currently registered under that id. Earlier
    // migration work could persist a globally-active Project under the selected
    // repo, and registry rows can also outlive a moved/re-added checkout. The
    // binding endpoint has already resolved the selected repo's real origin;
    // fail closed unless the canonical target agrees with it. This prevents a
    // stale Agentum binding from being rendered as a successful tracker for an
    // unrelated project.
    if !github
        .repository_slug
        .trim()
        .eq_ignore_ascii_case(resolved_slug.trim())
    {
        if config.provenance == ProjectTrackerProvenance::Migrated {
            // A migrated row is compatibility data, not an explicit user
            // choice. Remove the stale projection with CAS and immediately
            // re-run exact-slug migration for the repo currently registered
            // under this id. This repairs old global-fallback migrations once,
            // while preserving a concurrently-written config.
            match state
                .store
                .delete_project_tracker_config(repo_id, Some(config.revision))
                .await?
            {
                ProjectTrackerWrite::Written(_) => {
                    config = migrate(state, repo_id).await?.0.unwrap_or_else(|| {
                        unconfigured(repo_id, ProjectTrackerProvenance::Migrated)
                    });
                }
                ProjectTrackerWrite::Conflict(current) => return Err(conflict(current)),
            }
        } else {
            // Never silently overwrite an explicit configuration. Surface the
            // disagreement so clients can show a repair action instead of
            // claiming that the wrong Project is connected.
            return Err(ApiError::Custom(
                StatusCode::CONFLICT,
                serde_json::json!({
                    "error": {
                        "code": "tracker_target_mismatch",
                        "message": format!(
                            "configured tracker repository {} does not match selected repository {}",
                            github.repository_slug, resolved_slug
                        )
                    }
                }),
            ));
        }
    }
    let binding = config
        .github
        .filter(|target| {
            target
                .repository_slug
                .trim()
                .eq_ignore_ascii_case(resolved_slug.trim())
        })
        .and_then(|target| target.project_binding);
    Ok(binding.as_ref().map(from_canonical_binding))
}

pub(crate) async fn compatibility_put(
    state: &AppState,
    repo_id: &str,
    slug: String,
    binding: BoardBinding,
) -> Result<(), ApiError> {
    let current = state.store.get_project_tracker_config(repo_id).await?;
    let expected_revision = current.as_ref().map(|config| config.revision);
    let preferences = current
        .map(|config| config.task_preferences)
        .unwrap_or_default();
    let config = ProjectTrackerConfig {
        schema_version: PROJECT_TRACKER_SCHEMA_VERSION,
        repo_id: repo_id.to_string(),
        revision: expected_revision.unwrap_or_default(),
        provider: Some(ProjectTrackerProvider::Github),
        github: Some(ProjectTrackerGithubTarget {
            repository_slug: slug,
            project_binding: Some(to_canonical_binding(binding)),
        }),
        linear: None,
        task_preferences: preferences,
        provenance: ProjectTrackerProvenance::Configured,
    };
    match state
        .store
        .put_project_tracker_config(config, expected_revision)
        .await?
    {
        ProjectTrackerWrite::Written(_) => Ok(()),
        ProjectTrackerWrite::Conflict(current) => Err(conflict(current)),
    }
}

pub(crate) async fn compatibility_delete(state: &AppState, repo_id: &str) -> Result<(), ApiError> {
    let Some(config) = state.store.get_project_tracker_config(repo_id).await? else {
        return Ok(());
    };
    match state
        .store
        .delete_project_tracker_config(repo_id, Some(config.revision))
        .await?
    {
        ProjectTrackerWrite::Written(_) => Ok(()),
        ProjectTrackerWrite::Conflict(current) => Err(conflict(current)),
    }
}

async fn get_config(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<Json<GetResponse>, ApiError> {
    super::repos::resolve_repo_path(&repo_id)?;
    if let Some(config) = state.store.get_project_tracker_config(&repo_id).await? {
        return Ok(Json(GetResponse {
            config: Some(config),
            migration_conflict: None,
        }));
    }
    let (config, migration_conflict) = migrate(&state, &repo_id).await?;
    Ok(Json(GetResponse {
        config,
        migration_conflict,
    }))
}

async fn put_config(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Json(mut body): Json<PutRequest>,
) -> Result<Json<ProjectTrackerConfig>, ApiError> {
    super::repos::resolve_repo_path(&repo_id)?;
    body.config.provenance = ProjectTrackerProvenance::Configured;
    validate(&body.config, &repo_id)?;
    match state
        .store
        .put_project_tracker_config(body.config, body.expected_revision)
        .await?
    {
        ProjectTrackerWrite::Written(config) => Ok(Json(config)),
        ProjectTrackerWrite::Conflict(current) => Err(conflict(current)),
    }
}

async fn patch_preferences(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Json(body): Json<PreferencesRequest>,
) -> Result<Json<ProjectTrackerConfig>, ApiError> {
    let mut current = state
        .store
        .get_project_tracker_config(&repo_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("tracker config is not configured".into()))?;
    current.task_preferences = body.preferences;
    match state
        .store
        .put_project_tracker_config(current, Some(body.expected_revision))
        .await?
    {
        ProjectTrackerWrite::Written(config) => Ok(Json(config)),
        ProjectTrackerWrite::Conflict(current) => Err(conflict(current)),
    }
}

async fn delete_config(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    super::repos::resolve_repo_path(&repo_id)?;
    match state
        .store
        .delete_project_tracker_config(&repo_id, query.expected_revision)
        .await?
    {
        ProjectTrackerWrite::Written(_) => Ok(StatusCode::NO_CONTENT),
        ProjectTrackerWrite::Conflict(current) => Err(conflict(current)),
    }
}

/// Transition compatibility lookup. Exact slug matching is allowed only when
/// canonical ownership is unambiguous (or all matching mappings are equal).
pub(crate) async fn binding_for_transition(
    state: &AppState,
    project_repo_id: Option<&str>,
    slug: &str,
) -> Result<Option<BoardBinding>, String> {
    if let Some(repo_id) = project_repo_id {
        let config = state
            .store
            .get_project_tracker_config(repo_id)
            .await
            .map_err(|e| e.to_string())?;
        let github = config.and_then(|c| c.github);
        return match github {
            Some(target) if target.repository_slug.eq_ignore_ascii_case(slug) => {
                Ok(target.project_binding.as_ref().map(from_canonical_binding))
            }
            Some(_) => Err(format!(
                "project tracker target does not match ticket repository {slug}"
            )),
            None => Ok(None),
        };
    }
    let matches = state
        .store
        .find_project_trackers_by_github_slug(slug)
        .await
        .map_err(|e| e.to_string())?;
    let mut bindings = matches
        .into_iter()
        .filter_map(|c| c.github?.project_binding);
    let first = bindings.next();
    if bindings.any(|binding| Some(&binding) != first.as_ref()) {
        return Err(format!("ambiguous project tracker config for {slug}"));
    }
    Ok(first.as_ref().map(from_canonical_binding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};
    use std::ffi::OsString;
    use tokio::sync::broadcast;

    fn mapping(prefix: &str) -> StatusMapping {
        StatusMapping {
            todo: format!("{prefix}-todo"),
            in_progress: format!("{prefix}-doing"),
            in_review: String::new(),
            ready_to_test: format!("{prefix}-qa"),
            done: format!("{prefix}-done"),
            blocked: format!("{prefix}-blocked"),
        }
    }

    fn binding(prefix: &str) -> BoardBinding {
        BoardBinding {
            project_id: format!("{prefix}-project"),
            status_field_id: format!("{prefix}-status"),
            status_mapping: mapping(prefix),
            done_closes_issue: true,
            project_title: Some(format!("{prefix} board")),
            project_owner: Some("acme".into()),
            project_owner_type: Some("organization".into()),
            project_number: Some(7),
            option_names: None,
        }
    }

    fn github_config(repo_id: &str, slug: &str, binding: BoardBinding) -> ProjectTrackerConfig {
        ProjectTrackerConfig {
            schema_version: PROJECT_TRACKER_SCHEMA_VERSION,
            repo_id: repo_id.into(),
            revision: 0,
            provider: Some(ProjectTrackerProvider::Github),
            github: Some(ProjectTrackerGithubTarget {
                repository_slug: slug.into(),
                project_binding: Some(to_canonical_binding(binding)),
            }),
            linear: None,
            task_preferences: Default::default(),
            provenance: ProjectTrackerProvenance::Configured,
        }
    }

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        let store = agentum_store::Store::open(&path).await.unwrap();
        let (bus, _) = broadcast::channel(16);
        AppState::new(store, bus)
    }

    struct EnvRestore(Vec<(&'static str, Option<OsString>)>);

    impl EnvRestore {
        fn set(pairs: &[(&'static str, &std::path::Path)]) -> Self {
            let old = pairs
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();
            for (key, value) in pairs {
                // SAFETY: every server test that mutates process environment
                // takes the crate-wide TEST_ENV_LOCK for its full lifetime.
                unsafe { std::env::set_var(key, value) };
            }
            Self(old)
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                // SAFETY: the owning test still holds TEST_ENV_LOCK while this
                // guard restores the previous process environment.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    fn validation_rejects_cross_provider_targets() {
        let config = ProjectTrackerConfig {
            schema_version: 1,
            repo_id: "a".into(),
            revision: 0,
            provider: Some(ProjectTrackerProvider::Github),
            github: None,
            linear: None,
            task_preferences: Default::default(),
            provenance: ProjectTrackerProvenance::Configured,
        };
        assert!(
            validate(&config, "a")
                .unwrap_err()
                .to_string()
                .contains("github target")
        );
        assert!(
            validate(&config, "b")
                .unwrap_err()
                .to_string()
                .contains("repoId")
        );
    }

    #[tokio::test]
    async fn compatibility_crud_uses_only_the_repo_owned_row() {
        let state = fresh_state().await;
        compatibility_put(&state, "repo-a", "acme/widgets".into(), binding("a"))
            .await
            .unwrap();
        compatibility_put(&state, "repo-b", "acme/other".into(), binding("b"))
            .await
            .unwrap();

        assert_eq!(
            compatibility_get(&state, "repo-a", "ACME/WIDGETS")
                .await
                .unwrap()
                .unwrap()
                .project_id,
            "a-project"
        );
        compatibility_delete(&state, "repo-a").await.unwrap();
        assert!(
            state
                .store
                .get_project_tracker_config("repo-a")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            compatibility_get(&state, "repo-b", "acme/other")
                .await
                .unwrap()
                .unwrap()
                .project_id,
            "b-project"
        );
    }

    #[tokio::test]
    async fn compatibility_get_rejects_an_explicit_cross_repo_binding() {
        let state = fresh_state().await;
        state
            .store
            .put_project_tracker_config(
                github_config("xcode-theme", "mateo/agentum", binding("agentum")),
                None,
            )
            .await
            .unwrap();
        let before = state
            .store
            .get_project_tracker_config("xcode-theme")
            .await
            .unwrap();

        let error = compatibility_get(&state, "xcode-theme", "mateo/xcode-theme")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error}");
        assert_eq!(
            state
                .store
                .get_project_tracker_config("xcode-theme")
                .await
                .unwrap(),
            before,
            "explicit configuration remains byte-equivalent after refusal"
        );
        assert_eq!(
            compatibility_get(&state, "xcode-theme", "MATEO/AGENTUM")
                .await
                .unwrap()
                .unwrap()
                .project_id,
            "agentum-project"
        );
    }

    #[tokio::test]
    async fn transition_lookup_rejects_mismatch_and_ambiguous_bindings() {
        let state = fresh_state().await;
        for (repo, binding) in [("repo-a", binding("a")), ("repo-b", binding("b"))] {
            state
                .store
                .put_project_tracker_config(github_config(repo, "acme/widgets", binding), None)
                .await
                .unwrap();
        }
        let error = binding_for_transition(&state, None, "acme/widgets")
            .await
            .unwrap_err();
        assert!(
            error.contains("ambiguous project tracker config"),
            "{error}"
        );
        let error = binding_for_transition(&state, Some("repo-a"), "acme/elsewhere")
            .await
            .unwrap_err();
        assert!(error.contains("does not match"), "{error}");
        assert_eq!(
            binding_for_transition(&state, Some("repo-a"), "ACME/WIDGETS")
                .await
                .unwrap()
                .unwrap()
                .project_id,
            "a-project"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_migrates_exact_repo_binding_once_without_rewriting_registry() {
        let _lock = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let repo_path = dir.path().join("widgets");
        let binding_path = dir.path().join("github_projects.json");
        std::fs::create_dir_all(home.join(".agentum")).unwrap();
        std::fs::create_dir_all(&repo_path).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&repo_path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/acme/widgets.git"
                ])
                .current_dir(&repo_path)
                .status()
                .unwrap()
                .success()
        );
        let repo_id = "repo-migrate";
        let registry = serde_json::to_string_pretty(&serde_json::json!([{
            "id": repo_id,
            "path": repo_path,
            "displayName": "Widgets",
            "badgeColor": "#000000",
            "addedAt": 1,
            "kind": "git",
            "trackerProvider": "github",
            "unknownFutureField": { "preserved": true }
        }]))
        .unwrap();
        let registry_path = home.join(".agentum/repos.json");
        std::fs::write(&registry_path, format!("{registry}\n")).unwrap();
        crate::github_projects::upsert_binding_at(&binding_path, "acme/widgets", binding("legacy"))
            .unwrap();
        let _env = EnvRestore::set(&[
            ("HOME", &home),
            ("AGENTUM_GITHUB_PROJECTS_CONFIG", &binding_path),
        ]);
        let state = fresh_state().await;

        let Json(first) = get_config(State(state.clone()), Path(repo_id.into()))
            .await
            .unwrap();
        let first = first.config.unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.provenance, ProjectTrackerProvenance::Migrated);
        assert_eq!(first.github.unwrap().repository_slug, "acme/widgets");
        let Json(second) = get_config(State(state.clone()), Path(repo_id.into()))
            .await
            .unwrap();
        assert_eq!(
            second.config.unwrap().revision,
            1,
            "migration is idempotent"
        );
        assert_eq!(
            std::fs::read_to_string(registry_path).unwrap(),
            format!("{registry}\n"),
            "unknown repo fields remain byte-unchanged"
        );

        let stale = ProjectTrackerConfig {
            schema_version: PROJECT_TRACKER_SCHEMA_VERSION,
            repo_id: repo_id.into(),
            revision: 1,
            provider: Some(ProjectTrackerProvider::Github),
            github: Some(ProjectTrackerGithubTarget {
                repository_slug: "mateo/agentum".into(),
                project_binding: Some(to_canonical_binding(binding("agentum"))),
            }),
            linear: None,
            task_preferences: Default::default(),
            provenance: ProjectTrackerProvenance::Migrated,
        };
        assert!(matches!(
            state
                .store
                .put_project_tracker_config(stale, Some(1))
                .await
                .unwrap(),
            ProjectTrackerWrite::Written(_)
        ));
        assert_eq!(
            compatibility_get(&state, repo_id, "acme/widgets")
                .await
                .unwrap()
                .unwrap()
                .project_id,
            "legacy-project"
        );
        let repaired = state
            .store
            .get_project_tracker_config(repo_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repaired.provenance, ProjectTrackerProvenance::Migrated);
        assert_eq!(repaired.github.unwrap().repository_slug, "acme/widgets");
    }
}
