//! Workspace-provisioning routes (spec 010 F3) — the thin wire layer over
//! `crate::provision` (the domain core lives at crate root, the
//! `github_projects.rs` precedent).
//!
//! `POST /api/github/repo-from-template` — create-or-adopt a repo from a
//! template and clone it under a local directory.
//! `POST /api/workspace/provision` — the idempotent GitHub label and board
//! binding ensure.
//!
//! Provisioning is local-host only:
//! `expand_workdir` + `is_dir` guards; the only hard 4xx are request-shape /
//! missing-workdir errors — every provisioning step inside is best-effort and
//! per-step reported. Only the *slug resolution* half is host-aware
//! (spec 020 F1, via the shared `util::resolve_tracker_slug`); an SSH repoId
//! still dies at the local `is_dir` gate first, which is correct — remote
//! remote project provisioning remains out of scope. All routes are
//! authenticated.

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;
use crate::github_projects::StatusMapping;
use crate::provision::{ProjectChoice, ProvisionCtx, ProvisionReport, provision_repo};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/github/repo-from-template", post(repo_from_template))
        .route("/api/workspace/provision", post(provision_workspace))
}

// ─── Pure request validation (unit-tested) ──────────────────────────────────

/// A GitHub repo name is ONE path segment — it becomes `directory/<name>` on
/// disk, so separators/traversal must be unrepresentable at the wire.
fn validate_repo_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("`name` is required".into());
    }
    if name == "." || name == ".." {
        return Err("`name` must not be a dot path".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("`name` may only contain letters, digits, '-', '_' and '.'".into());
    }
    Ok(name)
}

/// An owner login: non-empty, no separators/whitespace (it rides an argv slug).
fn validate_owner(owner: &str) -> Result<&str, String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return Err("`owner` is required".into());
    }
    if owner.contains('/') || owner.chars().any(char::is_whitespace) {
        return Err("`owner` must be a bare GitHub login".into());
    }
    Ok(owner)
}

/// The template ref must be `owner/repo` — exactly one `/`, no whitespace.
fn validate_template(template: &str) -> Result<&str, String> {
    let template = template.trim();
    let ok = matches!(template.split('/').collect::<Vec<_>>().as_slice(),
        [owner, repo] if !owner.is_empty() && !repo.is_empty())
        && !template.chars().any(char::is_whitespace);
    if !ok {
        return Err("`templateRepo` must be `owner/repo`".into());
    }
    Ok(template)
}

/// `visibility`: absent/`private` → private (the safe default), `public` →
/// public, anything else is a request-shape 400. Returns `private?`.
fn validate_visibility(visibility: Option<&str>) -> Result<bool, String> {
    match visibility.map(str::trim) {
        None | Some("") | Some("private") => Ok(true),
        Some("public") => Ok(false),
        Some(other) => Err(format!(
            "`visibility` must be \"private\" or \"public\", not {other:?}"
        )),
    }
}

/// The wire `project` shape: `{owner, ownerType, number}` links an existing
/// board; `{create: true, owner, ownerType, title}` creates one first (D5).
/// Flattened + a pure converter (instead of an untagged enum) so the 400s
/// name exactly what's missing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectChoiceDto {
    #[serde(default)]
    create: bool,
    owner: String,
    owner_type: String,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    title: Option<String>,
}

fn project_choice(dto: &ProjectChoiceDto) -> Result<ProjectChoice, String> {
    let owner = validate_owner(&dto.owner).map_err(|e| format!("project.{e}"))?;
    let owner_type = dto.owner_type.trim();
    if owner_type.is_empty() {
        return Err("project.`ownerType` is required".into());
    }
    if dto.create {
        let title = dto.title.as_deref().map(str::trim).unwrap_or_default();
        if title.is_empty() {
            return Err("project.`title` is required to create a board".into());
        }
        Ok(ProjectChoice::Create {
            owner: owner.into(),
            owner_type: owner_type.into(),
            title: title.into(),
        })
    } else {
        let number = dto
            .number
            .ok_or("project.`number` is required to link a board (or set `create: true`)")?;
        Ok(ProjectChoice::Link {
            owner: owner.into(),
            owner_type: owner_type.into(),
            number,
        })
    }
}

/// The five option-ID selects on the wire — a local twin of the F1 route's
/// private DTO (that F1 file stays untouched). A PRESENT mapping must be
/// complete: a partial one is a request-shape 400 naming the blank phases
/// (the constructor invariant at this wire too).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStatusMapping {
    todo: String,
    in_progress: String,
    /// #379: optional In Review / PR column — empty folds onto In Progress.
    #[serde(default)]
    in_review: String,
    ready_to_test: String,
    done: String,
    blocked: String,
}

fn status_mapping_from_wire(dto: &WireStatusMapping) -> Result<StatusMapping, String> {
    let mut empty: Vec<&str> = Vec::new();
    for (name, value) in [
        ("todo", &dto.todo),
        ("inProgress", &dto.in_progress),
        ("readyToTest", &dto.ready_to_test),
        ("done", &dto.done),
        ("blocked", &dto.blocked),
    ] {
        if value.trim().is_empty() {
            empty.push(name);
        }
    }
    if !empty.is_empty() {
        return Err(format!(
            "statusMapping must map every phase to a non-empty option id; missing: {}",
            empty.join(", ")
        ));
    }
    Ok(StatusMapping {
        todo: dto.todo.trim().to_string(),
        in_progress: dto.in_progress.trim().to_string(),
        in_review: dto.in_review.trim().to_string(),
        ready_to_test: dto.ready_to_test.trim().to_string(),
        done: dto.done.trim().to_string(),
        blocked: dto.blocked.trim().to_string(),
    })
}

// ─── Handlers ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoFromTemplateRequest {
    owner: String,
    name: String,
    template_repo: String,
    directory: String,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoFromTemplateResponse {
    slug: String,
    path: String,
    created: bool,
}

/// `POST /api/github/repo-from-template` — spec 010 §5.1 template mode.
/// Idempotent: an existing local clone or remote repo is adopted, never
/// re-created. A `gh` failure (e.g. the template repo is not marked
/// "Template repository" on GitHub) surfaces its stderr VERBATIM as the 400
/// body — never silent, never rephrased.
async fn repo_from_template(
    State(_state): State<AppState>,
    Json(req): Json<RepoFromTemplateRequest>,
) -> Result<Json<RepoFromTemplateResponse>, ApiError> {
    let owner = validate_owner(&req.owner).map_err(ApiError::BadRequest)?;
    let name = validate_repo_name(&req.name).map_err(ApiError::BadRequest)?;
    let template = validate_template(&req.template_repo).map_err(ApiError::BadRequest)?;
    let private = validate_visibility(req.visibility.as_deref()).map_err(ApiError::BadRequest)?;
    let directory = super::util::expand_workdir(&req.directory)?;
    if !directory.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "directory does not exist: {}",
            directory.display()
        )));
    }
    let result = crate::provision::create_repo_from_template(
        &crate::github_projects::gh_bin(),
        owner,
        name,
        template,
        &directory,
        private,
    )
    .await
    .map_err(ApiError::BadRequest)?;
    Ok(Json(RepoFromTemplateResponse {
        slug: result.slug,
        path: result.path.to_string_lossy().into_owned(),
        created: result.created,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRequest {
    workdir: String,
    #[serde(default)]
    slug: Option<String>,
    /// Spec 020 F1: resolve the slug on this registered repo's host instead
    /// of the local one. Absent = local (pre-020 behavior byte-for-byte).
    #[serde(default)]
    repo_id: Option<String>,
    /// Absent = no board requested (an existing binding still short-circuits
    /// to "already bound").
    #[serde(default)]
    project: Option<ProjectChoiceDto>,
    #[serde(default)]
    status_mapping: Option<WireStatusMapping>,
    /// Absent = ON — D1's default via the binding type's one definition site.
    #[serde(default)]
    done_closes_issue: Option<bool>,
}

/// `POST /api/workspace/provision` — run the one idempotent ensure and return
/// the per-step report. Always 200 once the request shape and workdir pass:
/// step failures live INSIDE the report (warnings at the UI, never blockers).
async fn provision_workspace(
    State(state): State<AppState>,
    Json(req): Json<ProvisionRequest>,
) -> Result<Json<ProvisionReport>, ApiError> {
    let workdir = super::util::expand_workdir(&req.workdir)?;
    if !workdir.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }
    let slug = super::util::resolve_tracker_slug(
        &state,
        req.repo_id.as_deref(),
        &req.workdir,
        req.slug.as_deref(),
    )
    .await?;
    let project = match &req.project {
        Some(dto) => Some(project_choice(dto).map_err(ApiError::BadRequest)?),
        None => None,
    };
    let status_mapping = match &req.status_mapping {
        Some(wire) => Some(status_mapping_from_wire(wire).map_err(ApiError::BadRequest)?),
        None => None,
    };
    let report = provision_repo(ProvisionCtx {
        program: &crate::github_projects::gh_bin(),
        bindings_path: None,
        slug: &slug,
        project,
        status_mapping,
        done_closes_issue: req
            .done_closes_issue
            .unwrap_or_else(crate::github_projects::default_true),
        state_map: crate::task_sink::GithubStateMap::from_env(),
    })
    .await;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The link|create disambiguation + its named 400s: `create: true` needs a
    /// title, link needs a number — a shapeless project can't reach the core.
    #[test]
    fn project_choice_parses_link_and_create_and_rejects_malformed() {
        let link: ProjectChoiceDto = serde_json::from_value(serde_json::json!({
            "owner": "acme", "ownerType": "organization", "number": 7
        }))
        .unwrap();
        assert!(matches!(
            project_choice(&link).unwrap(),
            ProjectChoice::Link { number: 7, .. }
        ));

        let create: ProjectChoiceDto = serde_json::from_value(serde_json::json!({
            "create": true, "owner": "acme", "ownerType": "user", "title": "Board"
        }))
        .unwrap();
        assert!(matches!(
            project_choice(&create).unwrap(),
            ProjectChoice::Create { .. }
        ));

        // Link without a number names the miss (and the create escape hatch).
        let bad: ProjectChoiceDto =
            serde_json::from_value(serde_json::json!({ "owner": "acme", "ownerType": "user" }))
                .unwrap();
        let err = project_choice(&bad).unwrap_err();
        assert!(err.contains("number"), "{err}");

        // Create without a title names the miss.
        let bad: ProjectChoiceDto = serde_json::from_value(
            serde_json::json!({ "create": true, "owner": "acme", "ownerType": "user" }),
        )
        .unwrap();
        let err = project_choice(&bad).unwrap_err();
        assert!(err.contains("title"), "{err}");
    }

    /// `name` becomes `directory/<name>` on disk — traversal/separators are
    /// unrepresentable; visibility is a closed enum with a private default.
    #[test]
    fn repo_name_and_visibility_validation() {
        assert_eq!(validate_repo_name(" my-repo_1.x ").unwrap(), "my-repo_1.x");
        for bad in ["", ".", "..", "a/b", "a b", "a\\b", "../up"] {
            assert!(validate_repo_name(bad).is_err(), "{bad:?} must be rejected");
        }
        assert_eq!(validate_owner("acme").unwrap(), "acme");
        assert!(validate_owner("a/b").is_err());
        assert_eq!(validate_template("o/r").unwrap(), "o/r");
        for bad in ["", "o", "o/", "/r", "o/r/x", "o r/t"] {
            assert!(validate_template(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(validate_visibility(None).unwrap(), "default = private");
        assert!(validate_visibility(Some("private")).unwrap());
        assert!(!validate_visibility(Some("public")).unwrap());
        assert!(validate_visibility(Some("internal")).is_err());
    }

    /// The camelCase wire pin: the request DTOs are the contract the TS
    /// client mirrors; a partial statusMapping is a named 400.
    #[test]
    fn provision_request_wire_shape_and_partial_mapping_rejected() {
        let req: ProvisionRequest = serde_json::from_value(serde_json::json!({
            "workdir": "/tmp/repo",
            "project": { "create": true, "owner": "acme", "ownerType": "user", "title": "B" },
            "statusMapping": { "todo": "t", "inProgress": "i", "readyToTest": "r",
                               "done": "d", "blocked": "b" },
            "doneClosesIssue": false
        }))
        .unwrap();
        assert_eq!(req.done_closes_issue, Some(false));
        // Spec 020 F1: absent repoId → None (pre-020 requests byte-identical).
        assert_eq!(req.repo_id, None);
        let mapping = status_mapping_from_wire(req.status_mapping.as_ref().unwrap()).unwrap();
        assert_eq!(mapping.ready_to_test, "r");

        let with_repo: ProvisionRequest = serde_json::from_value(serde_json::json!({
            "workdir": "/tmp/repo",
            "repoId": "r-1"
        }))
        .unwrap();
        assert_eq!(with_repo.repo_id.as_deref(), Some("r-1"));

        let partial: WireStatusMapping = serde_json::from_value(serde_json::json!({
            "todo": "t", "inProgress": " ", "readyToTest": "r", "done": "d", "blocked": ""
        }))
        .unwrap();
        let err = status_mapping_from_wire(&partial).unwrap_err();
        assert!(
            err.contains("inProgress") && err.contains("blocked"),
            "{err}"
        );
    }
}
