//! Projects v2 board-binding routes (spec 010 F1).
//!
//! `POST /api/github/project-binding/discover` — one `gh api graphql` call
//! resolving a project's Status field + the fuzzy phase mapping (which doubles
//! as the `project`-scope probe, AC 2d).
//! `GET/PUT/DELETE /api/github/project-binding` — read/bind/unbind the
//! per-repo binding persisted in the server-owned `github_projects.json`.
//!
//! A separate module from `routes/github.rs` (the issue surface) — the
//! `git.rs`-decomposition precedent. All routes authed (no `is_public`
//! changes). Slug resolution reuses `board_goals::resolve_github_slug` via the
//! same `{workdir, slug?}` pattern every github.rs route uses; the file stays
//! snake_case while these DTOs are the camelCase wire twins.

use agentum_core::LOCAL_HOST_ID;
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;
use crate::github_projects::{
    self, BoardBinding, MatchVia, ResolvedMapping, StatusMapping, StatusNames, StatusOption,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/github/project-binding/discover",
            post(discover_binding),
        )
        .route(
            "/api/github/project-binding",
            get(get_binding).put(put_binding).delete(delete_binding),
        )
}

/// Resolve the repo slug the binding is keyed on — the exact
/// `{workdir, slug?}` contract of the sibling github.rs routes, including the
/// typed `no_github_repo` 422 the UI already branches on.
async fn resolve_slug(
    state: &AppState,
    workdir: &str,
    slug_hint: Option<&str>,
) -> Result<String, ApiError> {
    let workdir = workdir.trim();
    if workdir.is_empty() {
        return Err(ApiError::BadRequest("`workdir` is required".into()));
    }
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    super::board_goals::resolve_github_slug(&host, workdir, slug_hint)
        .await
        .map_err(|_| {
            ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": { "code": "no_github_repo", "message": "no GitHub repo resolved for this project" } }),
            )
        })
}

/// Map a classified discovery failure onto the wire: `scope_missing` rides the
/// typed 422 envelope (the `no_github_repo` precedent) so the UI can render
/// the `gh auth refresh -s project` remedy verbatim; every other kind is a
/// classified 400 with the same `{error: {code, message}}` shape.
fn projects_error_to_api(err: github_projects::ProjectsError) -> ApiError {
    let status = if err.kind == "scope_missing" {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::BAD_REQUEST
    };
    ApiError::Custom(
        status,
        json!({ "error": { "code": err.kind, "message": err.message } }),
    )
}

// ─── Wire DTOs (camelCase twins of the snake_case file shapes) ──────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoverRequest {
    owner: String,
    /// `"user"` | `"organization"` — the picker always supplies it
    /// (`gh_resolve_project_ref` resolves it for pasted shorthand).
    owner_type: String,
    number: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedPhaseDto {
    option_id: String,
    name: String,
    /// `"matched"` | `"fell_back"` — FellBack renders the D5 hint chip.
    via: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedMappingDto {
    todo: ResolvedPhaseDto,
    in_progress: ResolvedPhaseDto,
    ready_to_test: ResolvedPhaseDto,
    done: ResolvedPhaseDto,
    blocked: ResolvedPhaseDto,
}

fn resolved_phase_dto(p: &github_projects::ResolvedPhase) -> ResolvedPhaseDto {
    ResolvedPhaseDto {
        option_id: p.option_id.clone(),
        name: p.option_name.clone(),
        via: match p.via {
            MatchVia::Matched => "matched",
            MatchVia::FellBack => "fell_back",
        },
    }
}

fn resolved_mapping_dto(m: &ResolvedMapping) -> ResolvedMappingDto {
    ResolvedMappingDto {
        todo: resolved_phase_dto(&m.todo),
        in_progress: resolved_phase_dto(&m.in_progress),
        ready_to_test: resolved_phase_dto(&m.ready_to_test),
        done: resolved_phase_dto(&m.done),
        blocked: resolved_phase_dto(&m.blocked),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoverResponse {
    project_id: String,
    title: String,
    status_field_id: String,
    options: Vec<StatusOption>,
    /// `null` when the mapper refused (an unmappable core phase) — the UI
    /// renders empty selects + the refusal, never a partial pre-selection.
    resolved: Option<ResolvedMappingDto>,
    /// Snake_case phase ids (`todo` / `in_progress` / `done`) with no synonym
    /// match. Empty when `resolved` is present.
    unmapped_phases: Vec<&'static str>,
}

/// The five option-ID selects on the wire. Doubles as the `optionNames` shape
/// (five names) — identical five-field layout, different semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusMappingDto {
    todo: String,
    in_progress: String,
    ready_to_test: String,
    done: String,
    blocked: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingDto {
    project_id: String,
    status_field_id: String,
    status_mapping: StatusMappingDto,
    done_closes_issue: bool,
    project_title: Option<String>,
    project_owner: Option<String>,
    project_owner_type: Option<String>,
    project_number: Option<i64>,
    option_names: Option<StatusMappingDto>,
}

fn binding_dto(b: &BoardBinding) -> BindingDto {
    BindingDto {
        project_id: b.project_id.clone(),
        status_field_id: b.status_field_id.clone(),
        status_mapping: StatusMappingDto {
            todo: b.status_mapping.todo.clone(),
            in_progress: b.status_mapping.in_progress.clone(),
            ready_to_test: b.status_mapping.ready_to_test.clone(),
            done: b.status_mapping.done.clone(),
            blocked: b.status_mapping.blocked.clone(),
        },
        done_closes_issue: b.done_closes_issue,
        project_title: b.project_title.clone(),
        project_owner: b.project_owner.clone(),
        project_owner_type: b.project_owner_type.clone(),
        project_number: b.project_number,
        option_names: b.option_names.as_ref().map(|n| StatusMappingDto {
            todo: n.todo.clone(),
            in_progress: n.in_progress.clone(),
            ready_to_test: n.ready_to_test.clone(),
            done: n.done.clone(),
            blocked: n.blocked.clone(),
        }),
    }
}

/// Validate the PUT mapping: all five option IDs non-empty — the constructor
/// invariant enforced at the wire too (AC 1: an unmapped phase is
/// unrepresentable). Pure so the 400 gate is unit-tested.
fn validate_status_mapping(dto: &StatusMappingDto) -> Result<StatusMapping, String> {
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
        ready_to_test: dto.ready_to_test.trim().to_string(),
        done: dto.done.trim().to_string(),
        blocked: dto.blocked.trim().to_string(),
    })
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /api/github/project-binding/discover` — one GraphQL call resolving
/// the Status field + the fuzzy mapping. The call IS the scope probe: a
/// missing `project` scope classifies to the typed 422 with the remedy.
async fn discover_binding(
    State(_state): State<AppState>,
    Json(body): Json<DiscoverRequest>,
) -> Result<Json<DiscoverResponse>, ApiError> {
    let owner = body.owner.trim();
    if owner.is_empty() {
        return Err(ApiError::BadRequest("`owner` is required".into()));
    }
    let discovered = github_projects::discover_status_field(
        &github_projects::gh_bin(),
        owner,
        body.owner_type.trim(),
        body.number,
    )
    .await
    .map_err(projects_error_to_api)?;

    // A refusal is NOT an error at the route level (D7): the UI gets the
    // options + which phases missed, and prompts a manual mapping instead.
    let (resolved, unmapped_phases) =
        match github_projects::resolve_status_mapping(&discovered.options) {
            Ok(m) => (Some(resolved_mapping_dto(&m)), Vec::new()),
            Err(_) => (
                None,
                github_projects::unmapped_core_phases(&discovered.options),
            ),
        };
    Ok(Json(DiscoverResponse {
        project_id: discovered.project_id,
        title: discovered.project_title,
        status_field_id: discovered.status_field_id,
        options: discovered.options,
        resolved,
        unmapped_phases,
    }))
}

#[derive(Debug, Deserialize)]
pub struct BindingQuery {
    pub workdir: String,
    pub slug: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetBindingResponse {
    slug: String,
    binding: Option<BindingDto>,
}

/// `GET /api/github/project-binding?workdir=…&slug=…` — the repo's binding,
/// fresh from disk (`null` when unbound).
async fn get_binding(
    State(state): State<AppState>,
    Query(q): Query<BindingQuery>,
) -> Result<Json<GetBindingResponse>, ApiError> {
    let slug = resolve_slug(&state, &q.workdir, q.slug.as_deref()).await?;
    let binding = github_projects::binding_for_slug(&slug);
    Ok(Json(GetBindingResponse {
        slug,
        binding: binding.as_ref().map(binding_dto),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutBindingRequest {
    workdir: String,
    #[serde(default)]
    slug: Option<String>,
    project_id: String,
    status_field_id: String,
    status_mapping: StatusMappingDto,
    /// Absent = ON — D1's default materializes through the binding type's one
    /// definition site (`github_projects::default_true`).
    #[serde(default)]
    done_closes_issue: Option<bool>,
    #[serde(default)]
    project_title: Option<String>,
    #[serde(default)]
    project_owner: Option<String>,
    #[serde(default)]
    project_owner_type: Option<String>,
    #[serde(default)]
    project_number: Option<i64>,
    #[serde(default)]
    option_names: Option<StatusMappingDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PutBindingResponse {
    slug: String,
    binding: BindingDto,
}

/// `PUT /api/github/project-binding` — validate (five non-empty option IDs;
/// 400 otherwise) then upsert the slug's binding.
async fn put_binding(
    State(state): State<AppState>,
    Json(body): Json<PutBindingRequest>,
) -> Result<Json<PutBindingResponse>, ApiError> {
    if body.project_id.trim().is_empty() {
        return Err(ApiError::BadRequest("`projectId` is required".into()));
    }
    if body.status_field_id.trim().is_empty() {
        return Err(ApiError::BadRequest("`statusFieldId` is required".into()));
    }
    let status_mapping =
        validate_status_mapping(&body.status_mapping).map_err(ApiError::BadRequest)?;
    let slug = resolve_slug(&state, &body.workdir, body.slug.as_deref()).await?;
    let binding = BoardBinding {
        project_id: body.project_id.trim().to_string(),
        status_field_id: body.status_field_id.trim().to_string(),
        status_mapping,
        done_closes_issue: body
            .done_closes_issue
            .unwrap_or_else(github_projects::default_true),
        project_title: body.project_title.clone(),
        project_owner: body.project_owner.clone(),
        project_owner_type: body.project_owner_type.clone(),
        project_number: body.project_number,
        option_names: body.option_names.as_ref().map(|n| StatusNames {
            todo: n.todo.clone(),
            in_progress: n.in_progress.clone(),
            ready_to_test: n.ready_to_test.clone(),
            done: n.done.clone(),
            blocked: n.blocked.clone(),
        }),
    };
    github_projects::upsert_binding(&slug, binding.clone()).map_err(ApiError::Internal)?;
    Ok(Json(PutBindingResponse {
        slug,
        binding: binding_dto(&binding),
    }))
}

/// `DELETE /api/github/project-binding?workdir=…&slug=…` — unbind (204 whether
/// or not a binding existed; delete is idempotent).
async fn delete_binding(
    State(state): State<AppState>,
    Query(q): Query<BindingQuery>,
) -> Result<StatusCode, ApiError> {
    let slug = resolve_slug(&state, &q.workdir, q.slug.as_deref()).await?;
    github_projects::remove_binding(&slug).map_err(ApiError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(todo: &str) -> StatusMappingDto {
        StatusMappingDto {
            todo: todo.into(),
            in_progress: "i".into(),
            ready_to_test: "r".into(),
            done: "d".into(),
            blocked: "b".into(),
        }
    }

    /// The wire-side constructor invariant (AC 1): any blank option id is a
    /// refusal naming the phase — a partial binding can't enter the file.
    #[test]
    fn put_binding_rejects_empty_phase_option() {
        let err = validate_status_mapping(&mapping("")).unwrap_err();
        assert!(err.contains("todo"), "names the blank phase: {err}");

        let mut two_blank = mapping("t");
        two_blank.ready_to_test = "  ".into();
        two_blank.blocked = String::new();
        let err = validate_status_mapping(&two_blank).unwrap_err();
        assert!(
            err.contains("readyToTest") && err.contains("blocked"),
            "{err}"
        );

        // All five present → the trimmed StatusMapping.
        let ok = validate_status_mapping(&mapping(" t ")).unwrap();
        assert_eq!(ok.todo, "t");
        assert_eq!(ok.blocked, "b");
    }

    /// Pin the camelCase request/response wire shapes (the DTOs are the
    /// contract the TS client mirrors).
    #[test]
    fn wire_shapes_are_camel_case() {
        let req: PutBindingRequest = serde_json::from_value(serde_json::json!({
            "workdir": "/tmp/repo",
            "projectId": "PVT_1",
            "statusFieldId": "PVTSSF_1",
            "statusMapping": {
                "todo": "t", "inProgress": "i", "readyToTest": "r",
                "done": "d", "blocked": "b"
            },
            "projectOwnerType": "organization",
            "projectNumber": 7
        }))
        .unwrap();
        assert_eq!(req.project_id, "PVT_1");
        assert_eq!(req.status_mapping.in_progress, "i");
        assert_eq!(req.done_closes_issue, None, "absent knob stays None → ON");

        let discover: DiscoverRequest = serde_json::from_value(serde_json::json!({
            "owner": "acme", "ownerType": "user", "number": 3
        }))
        .unwrap();
        assert_eq!(discover.owner_type, "user");

        let dto = binding_dto(&BoardBinding {
            project_id: "PVT_1".into(),
            status_field_id: "PVTSSF_1".into(),
            status_mapping: StatusMapping {
                todo: "t".into(),
                in_progress: "i".into(),
                ready_to_test: "r".into(),
                done: "d".into(),
                blocked: "b".into(),
            },
            done_closes_issue: true,
            project_title: None,
            project_owner: None,
            project_owner_type: None,
            project_number: None,
            option_names: None,
        });
        let wire = serde_json::to_value(&dto).unwrap();
        assert_eq!(wire["statusMapping"]["readyToTest"], "r");
        assert_eq!(wire["doneClosesIssue"], true);
    }

    /// The discover response's refusal shape: `resolved: null` +
    /// `unmappedPhases` (snake_case ids) — what the UI's manual-mapping
    /// prompt keys off.
    #[test]
    fn discover_response_serializes_refusal_shape() {
        let resp = DiscoverResponse {
            project_id: "PVT_1".into(),
            title: "T".into(),
            status_field_id: "F".into(),
            options: vec![StatusOption {
                id: "x".into(),
                name: "Weird".into(),
            }],
            resolved: None,
            unmapped_phases: vec!["todo", "in_progress", "done"],
        };
        let wire = serde_json::to_value(&resp).unwrap();
        assert!(wire["resolved"].is_null());
        assert_eq!(wire["unmappedPhases"][1], "in_progress");
        assert_eq!(wire["options"][0]["name"], "Weird");
    }
}
