//! GitHub ProjectV2 surface (view reads + item field mutations), served by
//! shelling out to `gh api graphql`.
//!
//! Why `gh api graphql` instead of a bespoke REST/GraphQL HTTP client: it reuses
//! the user's existing `gh` login (keyring or env token), so there is no token to
//! store, refresh, or leak in this process — the same trade-off `gh.rs` already
//! makes for `gh issue/pr list`. ProjectV2 reads require the `read:project` OAuth
//! scope; when the token lacks it, GitHub answers with an `INSUFFICIENT_SCOPES`
//! GraphQL error, which we classify as `scope_missing` so the renderer's
//! GhAuthErrorHelp guides `gh auth refresh -s read:project` instead of a dead end.
//! Field mutations (drag-between-columns, table cell edits) additionally need the
//! `project` write scope — same classification, GitHub's message names the scope.
//!
//! The pure `map_*` / `classify_*` / `parse_*` / `select_view` /
//! `field_mutation_value` helpers are unit tested against representative GraphQL
//! JSON; the `graphql()` subprocess wrapper is the only impure seam.

use serde_json::{json, Value};

// Hard cap on rows fetched for one table read. ProjectV2.items has no server-side
// view-filter, so we page through the whole project; beyond this we return the
// renderer's `too_large` state (with totalCount) rather than spin on a huge board.
const MAX_ITEMS: usize = 500;
const PAGE_SIZE: u32 = 50;

// ─── Errors ──────────────────────────────────────────────────────────────

// A classified failure shaped like the renderer's GitHubProjectViewError.
#[derive(Debug)]
struct ProjectError {
    kind: &'static str,
    message: String,
    code: Option<String>,
}

impl ProjectError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        ProjectError {
            kind,
            message: message.into(),
            code: None,
        }
    }

    fn with_code(kind: &'static str, message: impl Into<String>, code: impl Into<String>) -> Self {
        ProjectError {
            kind,
            message: message.into(),
            code: Some(code.into()),
        }
    }

    // The `{ ok:false, error }` envelope every ProjectV2 result type shares.
    fn envelope(&self) -> Value {
        let mut error = json!({ "type": self.kind, "message": self.message });
        if let Some(code) = &self.code {
            error["details"] = json!({ "code": code });
        }
        json!({ "ok": false, "error": error })
    }
}

// Map a GraphQL `errors[]` array onto a ProjectError. ProjectV2's most common
// failure is INSUFFICIENT_SCOPES (token without read:project); NOT_FOUND covers a
// bad owner/number or a private project the token can't see.
fn classify_graphql_errors(errors: &[Value]) -> ProjectError {
    let first = errors.first();
    let typ = first
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = first
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("GitHub Projects request failed.")
        .to_string();
    let kind = match typ {
        "INSUFFICIENT_SCOPES" => "scope_missing",
        "NOT_FOUND" => "not_found",
        "FORBIDDEN" => "scope_missing",
        "RATE_LIMITED" => "rate_limited",
        _ => {
            let lower = message.to_lowercase();
            if lower.contains("read:project") || lower.contains("required scopes") {
                "scope_missing"
            } else if lower.contains("could not resolve to") || lower.contains("not found") {
                "not_found"
            } else {
                "unknown"
            }
        }
    };
    ProjectError::with_code(kind, message, typ)
}

// `gh` failed before returning a GraphQL body (not installed, not logged in,
// network). Reuse gh.rs's stderr heuristics so the renderer gets a coherent kind.
fn classify_stderr(stderr: &str) -> ProjectError {
    let lower = stderr.to_lowercase();
    let kind = if lower.contains("read:project") || lower.contains("required scopes") {
        "scope_missing"
    } else if lower.contains("gh auth login")
        || lower.contains("not logged in")
        || lower.contains("authentication")
    {
        "auth_required"
    } else if lower.contains("could not resolve to") || lower.contains("not found") {
        "not_found"
    } else if lower.contains("rate limit") {
        "rate_limited"
    } else if lower.contains("could not connect") || lower.contains("timeout") {
        "network_error"
    } else {
        "unknown"
    };
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Couldn't reach GitHub Projects.")
        .to_string();
    ProjectError::new(kind, message)
}

// ─── GraphQL runner ─────────────────────────────────────────────────────

#[derive(Debug)]
enum Scalar {
    Str(String),
    Int(i64),
}

// Run one GraphQL operation through `gh api graphql`. String vars go through `-f`
// (always a string); Int vars through `-F` (typed) so `$number:Int!` binds as a
// number. The owner login is ALWAYS a `$var` — never interpolated into the query
// string — so a hostile login can't inject GraphQL.
async fn graphql(query: &str, vars: &[(&str, Scalar)]) -> Result<Value, ProjectError> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.arg("api").arg("graphql");
    cmd.arg("-f").arg(format!("query={query}"));
    for (key, value) in vars {
        match value {
            Scalar::Str(s) => {
                cmd.arg("-f").arg(format!("{key}={s}"));
            }
            Scalar::Int(n) => {
                cmd.arg("-F").arg(format!("{key}={n}"));
            }
        }
    }

    let output = cmd.output().await.map_err(|_| {
        ProjectError::new(
            "auth_required",
            "GitHub CLI (`gh`) is not installed or not on PATH.",
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // gh prints the JSON body on both success and GraphQL errors; parse it first.
    if let Ok(body) = serde_json::from_str::<Value>(&stdout) {
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                return Err(classify_graphql_errors(errors));
            }
        }
        match body.get("data") {
            Some(data) if !data.is_null() => return Ok(data.clone()),
            _ => {
                return Err(ProjectError::new(
                    "unknown",
                    "GitHub returned an empty response.",
                ))
            }
        }
    }
    // No JSON body → gh failed before the request (auth/scope/network).
    Err(classify_stderr(&String::from_utf8_lossy(&output.stderr)))
}

// 'organization' / 'user' are validated enum values from the renderer, safe to
// interpolate as the GraphQL root field. Anything unexpected falls back to `user`.
fn owner_node(owner_type: &str) -> &'static str {
    if owner_type == "organization" {
        "organization"
    } else {
        "user"
    }
}

// ─── GraphQL selection fragments ──────────────────────────────────────────

// id/name/dataType come from the ProjectV2FieldCommon interface (all members
// implement it); options/configuration are the single-select / iteration extras.
const FIELD_SELECTION: &str = r#"
  __typename
  ... on ProjectV2FieldCommon { id name dataType }
  ... on ProjectV2SingleSelectField { options { id name color } }
  ... on ProjectV2IterationField {
    configuration {
      iterations { id title startDate duration }
      completedIterations { id title startDate duration }
    }
  }
"#;

const FIELD_VALUE_SELECTION: &str = r#"
  __typename
  ... on ProjectV2ItemFieldTextValue { text field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldNumberValue { number field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldDateValue { date field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldSingleSelectValue { optionId name color field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldIterationValue { iterationId title startDate duration field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldLabelValue { labels(first: 20) { nodes { name color } } field { ... on ProjectV2FieldCommon { id } } }
  ... on ProjectV2ItemFieldUserValue { users(first: 20) { nodes { login name avatarUrl } } field { ... on ProjectV2FieldCommon { id } } }
"#;

const ASSIGNEES_LABELS: &str = r#"
  repository { nameWithOwner }
  assignees(first: 10) { nodes { login name avatarUrl } }
  labels(first: 20) { nodes { name color } }
"#;

// `parent` (sub-issues) and `issueType` are newer Issue fields; on tokens/repos
// where they aren't available the whole query fails validation. We try with them
// and fall back to `content_selection(false)` on that error (parentFieldDropped).
fn content_selection(with_parent: bool) -> String {
    let issue_extra = if with_parent {
        "parent { number title url } issueType { id name color description }"
    } else {
        ""
    };
    format!(
        r#"
        __typename
        ... on Issue {{
          number title body url state stateReason
          {ASSIGNEES_LABELS}
          {issue_extra}
        }}
        ... on PullRequest {{
          number title body url state isDraft
          {ASSIGNEES_LABELS}
        }}
        ... on DraftIssue {{
          title body
          assignees(first: 10) {{ nodes {{ login name avatarUrl }} }}
        }}
        "#
    )
}

// ─── Pure helpers ─────────────────────────────────────────────────────────

fn str_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str(value: &Value, key: &str) -> Value {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|s| json!(s))
        .unwrap_or(Value::Null)
}

// Map a connection's `nodes[]` through `f`; missing/empty connections → [].
fn nodes_map<F: Fn(&Value) -> Value>(conn: Option<&Value>, f: F) -> Vec<Value> {
    conn.and_then(|c| c.get("nodes"))
        .and_then(Value::as_array)
        .map(|nodes| nodes.iter().map(f).collect())
        .unwrap_or_default()
}

fn user_obj(user: &Value) -> Value {
    json!({
        "login": str_at(user, "login"),
        "name": opt_str(user, "name"),
        "avatarUrl": opt_str(user, "avatarUrl"),
    })
}

fn label_obj(label: &Value) -> Value {
    json!({ "name": str_at(label, "name"), "color": str_at(label, "color") })
}

fn iteration_obj(it: &Value, completed: bool) -> Value {
    json!({
        "id": str_at(it, "id"),
        "title": str_at(it, "title"),
        "startDate": str_at(it, "startDate"),
        "duration": it.get("duration").and_then(Value::as_i64).unwrap_or(0),
        "completed": completed,
    })
}

// A ProjectV2 field config → the renderer's discriminated GitHubProjectField.
fn map_field(node: &Value) -> Value {
    let id = str_at(node, "id");
    let name = str_at(node, "name");
    let data_type = node.get("dataType").and_then(Value::as_str).unwrap_or("");
    match data_type {
        "SINGLE_SELECT" => json!({
            "kind": "single-select",
            "id": id,
            "name": name,
            "dataType": "SINGLE_SELECT",
            "options": node
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| opts.iter().map(|o| json!({
                    "id": str_at(o, "id"),
                    "name": str_at(o, "name"),
                    "color": str_at(o, "color"),
                })).collect::<Vec<_>>())
                .unwrap_or_default(),
        }),
        "ITERATION" => {
            let mut iterations = Vec::new();
            if let Some(cfg) = node.get("configuration") {
                for it in cfg
                    .get("iterations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    iterations.push(iteration_obj(it, false));
                }
                for it in cfg
                    .get("completedIterations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    iterations.push(iteration_obj(it, true));
                }
            }
            json!({
                "kind": "iteration",
                "id": id,
                "name": name,
                "dataType": "ITERATION",
                "iterations": iterations,
            })
        }
        // TITLE/TEXT/NUMBER/DATE/ASSIGNEES/LABELS/… all share the plain shape; an
        // unknown dataType is preserved verbatim (the renderer renders it empty).
        _ => json!({ "kind": "field", "id": id, "name": name, "dataType": data_type }),
    }
}

fn field_id_of(node: &Value) -> Option<String> {
    node.get("field")
        .and_then(|f| f.get("id"))
        .and_then(Value::as_str)
        .map(String::from)
}

// A ProjectV2ItemFieldValue → (fieldId, GitHubProjectFieldValue). Returns None for
// value types outside the renderer's union (milestone, reviewers, repository, …)
// so they collapse to empty cells, per the contract's "never throw on unknown".
fn map_field_value(node: &Value) -> Option<(String, Value)> {
    let typename = node.get("__typename").and_then(Value::as_str)?;
    let field_id = field_id_of(node)?;
    let value = match typename {
        "ProjectV2ItemFieldTextValue" => {
            json!({ "kind": "text", "fieldId": field_id, "text": str_at(node, "text") })
        }
        "ProjectV2ItemFieldNumberValue" => json!({
            "kind": "number",
            "fieldId": field_id,
            "number": node.get("number").and_then(Value::as_f64).unwrap_or(0.0),
        }),
        "ProjectV2ItemFieldDateValue" => {
            json!({ "kind": "date", "fieldId": field_id, "date": str_at(node, "date") })
        }
        "ProjectV2ItemFieldSingleSelectValue" => json!({
            "kind": "single-select",
            "fieldId": field_id,
            "optionId": str_at(node, "optionId"),
            "name": str_at(node, "name"),
            "color": str_at(node, "color"),
        }),
        "ProjectV2ItemFieldIterationValue" => json!({
            "kind": "iteration",
            "fieldId": field_id,
            "iterationId": str_at(node, "iterationId"),
            "title": str_at(node, "title"),
            "startDate": str_at(node, "startDate"),
            "duration": node.get("duration").and_then(Value::as_i64).unwrap_or(0),
        }),
        "ProjectV2ItemFieldLabelValue" => json!({
            "kind": "labels",
            "fieldId": field_id,
            "labels": nodes_map(node.get("labels"), label_obj),
        }),
        "ProjectV2ItemFieldUserValue" => json!({
            "kind": "users",
            "fieldId": field_id,
            "users": nodes_map(node.get("users"), user_obj),
        }),
        _ => return None,
    };
    Some((field_id, value))
}

fn parent_issue(content: &Value) -> Value {
    match content.get("parent").filter(|p| !p.is_null()) {
        Some(parent) => json!({
            "number": parent.get("number").and_then(Value::as_i64).unwrap_or(0),
            "title": str_at(parent, "title"),
            "url": str_at(parent, "url"),
        }),
        None => Value::Null,
    }
}

fn issue_type(content: &Value) -> Value {
    match content.get("issueType").filter(|t| !t.is_null()) {
        Some(t) => json!({
            "id": str_at(t, "id"),
            "name": str_at(t, "name"),
            "color": opt_str(t, "color"),
            "description": opt_str(t, "description"),
        }),
        None => Value::Null,
    }
}

fn map_content(content: &Value) -> Value {
    let typename = content
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or("");
    // isDraft is meaningful only for PRs; null for issues/drafts.
    let is_draft = if typename == "PullRequest" {
        content
            .get("isDraft")
            .and_then(Value::as_bool)
            .map(Value::from)
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    json!({
        "number": content.get("number").and_then(Value::as_i64).map(Value::from).unwrap_or(Value::Null),
        "title": str_at(content, "title"),
        "body": opt_str(content, "body"),
        "url": opt_str(content, "url"),
        "state": opt_str(content, "state"),
        "stateReason": opt_str(content, "stateReason"),
        "isDraft": is_draft,
        "repository": content
            .get("repository")
            .and_then(|r| r.get("nameWithOwner"))
            .and_then(Value::as_str)
            .map(|s| json!(s))
            .unwrap_or(Value::Null),
        "assignees": nodes_map(content.get("assignees"), user_obj),
        "labels": nodes_map(content.get("labels"), label_obj),
        "parentIssue": parent_issue(content),
        "issueType": issue_type(content),
    })
}

// One ProjectV2Item → a GitHubProjectRow. `position` is the zero-based fetch index
// (the renderer's final sort tie-break).
fn map_row(item: &Value, position: usize) -> Value {
    let mut field_values = serde_json::Map::new();
    for fv in item
        .get("fieldValues")
        .and_then(|f| f.get("nodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some((field_id, value)) = map_field_value(fv) {
            field_values.insert(field_id, value);
        }
    }
    let content = item.get("content").cloned().unwrap_or(Value::Null);
    json!({
        "id": str_at(item, "id"),
        "itemType": item.get("type").and_then(Value::as_str).unwrap_or("REDACTED"),
        "content": map_content(&content),
        "fieldValuesByFieldId": Value::Object(field_values),
        "updatedAt": str_at(item, "updatedAt"),
        "position": position,
    })
}

// A ProjectV2View → the renderer's GitHubProjectView. `verticalGroupByFields`
// is GitHub's model of a Board view's columns (usually Status) and drives the
// Kanban renderer; `groupByFields` drives Table group headers (on a Board it is
// the optional swimlane grouping, typically empty). Both are queried and
// mapped. `sortByFields` stays empty on purpose: leaving it unset makes rows
// fall back to item `position` (GitHub's manual board order, which is exactly
// right within a board column) and avoids changing Table view ordering.
fn map_view(node: &Value) -> Value {
    json!({
        "id": str_at(node, "id"),
        "number": node.get("number").and_then(Value::as_i64).unwrap_or(0),
        "name": str_at(node, "name"),
        "layout": node.get("layout").and_then(Value::as_str).unwrap_or("TABLE_LAYOUT"),
        "filter": node.get("filter").and_then(Value::as_str).unwrap_or(""),
        "fields": nodes_map(node.get("fields"), map_field),
        "groupByFields": nodes_map(node.get("groupByFields"), map_field),
        "verticalGroupByFields": nodes_map(node.get("verticalGroupByFields"), map_field),
        "sortByFields": [],
    })
}

fn map_view_summary(node: &Value) -> Value {
    json!({
        "id": str_at(node, "id"),
        "number": node.get("number").and_then(Value::as_i64).unwrap_or(0),
        "name": str_at(node, "name"),
        "layout": node.get("layout").and_then(Value::as_str).unwrap_or("TABLE_LAYOUT"),
    })
}

// View-selection precedence: viewId > viewNumber > viewName > first TABLE_LAYOUT >
// first view. Operates on already-mapped GitHubProjectView values.
fn select_view(
    views: &[Value],
    view_id: Option<&str>,
    view_number: Option<i64>,
    view_name: Option<&str>,
) -> Option<Value> {
    if let Some(id) = view_id {
        if let Some(v) = views
            .iter()
            .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        {
            return Some(v.clone());
        }
    }
    if let Some(number) = view_number {
        if let Some(v) = views
            .iter()
            .find(|v| v.get("number").and_then(Value::as_i64) == Some(number))
        {
            return Some(v.clone());
        }
    }
    if let Some(name) = view_name {
        if let Some(v) = views
            .iter()
            .find(|v| v.get("name").and_then(Value::as_str) == Some(name))
        {
            return Some(v.clone());
        }
    }
    views
        .iter()
        .find(|v| v.get("layout").and_then(Value::as_str) == Some("TABLE_LAYOUT"))
        .or_else(|| views.first())
        .cloned()
}

// Parse the renderer's freeform project ref. Accepts:
//   - github.com/orgs/{login}/projects/{n}[/views/{m}]   → organization
//   - github.com/users/{login}/projects/{n}[/views/{m}]  → user
//   - {login}/{n}                                         → unknown owner type
// Returns (owner_type, owner, number, view_number). owner_type is None when it
// can't be inferred (the caller probes organization then user).
fn parse_project_ref(input: &str) -> Option<(Option<&'static str>, String, i64, Option<i64>)> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed
        .split_once("github.com/")
        .map(|(_, rest)| rest.trim_end_matches('/'))
    {
        let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        // orgs/{login}/projects/{n}[...]  or  users/{login}/projects/{n}[...]
        if segs.len() >= 4 && (segs[0] == "orgs" || segs[0] == "users") && segs[2] == "projects" {
            let owner_type = if segs[0] == "orgs" {
                "organization"
            } else {
                "user"
            };
            let number = segs[3].parse::<i64>().ok()?;
            let view_number = (segs.len() >= 6 && segs[4] == "views")
                .then(|| segs[5].parse::<i64>().ok())
                .flatten();
            return Some((Some(owner_type), segs[1].to_string(), number, view_number));
        }
        return None;
    }
    // Shorthand owner/number (owner type unknown).
    let (owner, number) = trimmed.split_once('/')?;
    let number = number.trim().parse::<i64>().ok()?;
    (!owner.trim().is_empty()).then(|| (None, owner.trim().to_string(), number, None))
}

// ─── Commands ──────────────────────────────────────────────────────────────

// `gh_resolve_project_ref` — owner/number or project URL → { owner, ownerType,
// number, title, viewNumber? }. When the URL didn't reveal the owner type, probe
// organization first, then user.
#[tauri::command]
pub async fn gh_resolve_project_ref(input: String) -> Value {
    let Some((owner_type, owner, number, view_number)) = parse_project_ref(&input) else {
        return ProjectError::new(
            "not_found",
            "Couldn't parse that project reference. Use a project URL or owner/number.",
        )
        .envelope();
    };

    let candidates: Vec<&str> = match owner_type {
        Some(node) => vec![node],
        None => vec!["organization", "user"],
    };

    let mut last_error = ProjectError::new("not_found", "Project not found.");
    for node in candidates {
        let query = format!(
            "query($owner: String!, $number: Int!) {{ {node}(login: $owner) {{ projectV2(number: $number) {{ id title }} }} }}"
        );
        let vars = [
            ("owner", Scalar::Str(owner.clone())),
            ("number", Scalar::Int(number)),
        ];
        match graphql(&query, &vars).await {
            Ok(data) => {
                let project = data.get(node).and_then(|n| n.get("projectV2"));
                if let Some(project) = project.filter(|p| !p.is_null()) {
                    let owner_type_out = if node == "organization" {
                        "organization"
                    } else {
                        "user"
                    };
                    let mut result = json!({
                        "ok": true,
                        "owner": owner,
                        "ownerType": owner_type_out,
                        "number": number,
                        "title": str_at(project, "title"),
                    });
                    if let Some(view) = view_number {
                        result["viewNumber"] = json!(view);
                    }
                    return result;
                }
                // Node resolved but projectV2 was null → keep probing the next.
                last_error = ProjectError::new("not_found", "Project not found.");
            }
            Err(err) => {
                // A scope/auth error won't improve by probing the other node type.
                if err.kind == "scope_missing" || err.kind == "auth_required" {
                    return err.envelope();
                }
                last_error = err;
            }
        }
    }
    last_error.envelope()
}

// `gh_list_accessible_projects` — the viewer's own projects plus those of the orgs
// they belong to. An org that errors becomes a partialFailure, not a hard failure.
#[tauri::command]
pub async fn gh_list_accessible_projects() -> Value {
    let query = r#"
        query {
          viewer {
            login
            projectsV2(first: 50) { nodes { id number title url } }
            organizations(first: 25) {
              nodes {
                login
                projectsV2(first: 50) { nodes { id number title url } }
              }
            }
          }
        }
    "#;
    let data = match graphql(query, &[]).await {
        Ok(data) => data,
        Err(err) => return err.envelope(),
    };

    let viewer = data.get("viewer").cloned().unwrap_or(Value::Null);
    let viewer_login = str_at(&viewer, "login");
    let mut projects: Vec<Value> = Vec::new();

    for node in viewer
        .get("projectsV2")
        .and_then(|p| p.get("nodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        projects.push(project_summary(node, &viewer_login, "user", "viewer"));
    }

    for org in viewer
        .get("organizations")
        .and_then(|o| o.get("nodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let org_login = str_at(org, "login");
        let source = format!("org:{org_login}");
        for node in org
            .get("projectsV2")
            .and_then(|p| p.get("nodes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            projects.push(project_summary(node, &org_login, "organization", &source));
        }
    }

    json!({ "ok": true, "projects": projects })
}

fn project_summary(node: &Value, owner: &str, owner_type: &str, source: &str) -> Value {
    json!({
        "id": str_at(node, "id"),
        "owner": owner,
        "ownerType": owner_type,
        "number": node.get("number").and_then(Value::as_i64).unwrap_or(0),
        "title": str_at(node, "title"),
        "url": str_at(node, "url"),
        "source": source,
    })
}

// `gh_list_project_views` — the views of one project (id/number/name/layout).
#[tauri::command]
pub async fn gh_list_project_views(
    owner: String,
    owner_type: String,
    project_number: i64,
) -> Value {
    let node = owner_node(&owner_type);
    let query = format!(
        "query($owner: String!, $number: Int!) {{ {node}(login: $owner) {{ projectV2(number: $number) {{ views(first: 50) {{ nodes {{ id number name layout }} }} }} }} }}"
    );
    let vars = [
        ("owner", Scalar::Str(owner)),
        ("number", Scalar::Int(project_number)),
    ];
    let data = match graphql(&query, &vars).await {
        Ok(data) => data,
        Err(err) => return err.envelope(),
    };
    let project = data.get(node).and_then(|n| n.get("projectV2"));
    let Some(project) = project.filter(|p| !p.is_null()) else {
        return ProjectError::new("not_found", "Project not found.").envelope();
    };
    let views = nodes_map(project.get("views"), map_view_summary);
    json!({ "ok": true, "views": views })
}

// `gh_get_project_view_table` — the table the user opens: project meta, the
// selected view's columns, and the (paginated) items with their field values.
#[tauri::command]
pub async fn gh_get_project_view_table(
    owner: String,
    owner_type: String,
    project_number: i64,
    view_id: Option<String>,
    view_number: Option<i64>,
    view_name: Option<String>,
    query_override: Option<String>,
) -> Value {
    // queryOverride (ephemeral filter) isn't applied server-side yet — ProjectV2's
    // items connection has no filter arg. Accepted and ignored for this pass.
    let _ = query_override;
    let node = owner_node(&owner_type);

    // 1) Project meta + views (with each view's column fields).
    let meta_query = format!(
        r#"
        query($owner: String!, $number: Int!) {{
          {node}(login: $owner) {{
            projectV2(number: $number) {{
              id number title url
              views(first: 50) {{
                nodes {{
                  id number name layout filter
                  fields(first: 50) {{ nodes {{ {FIELD_SELECTION} }} }}
                  groupByFields(first: 20) {{ nodes {{ {FIELD_SELECTION} }} }}
                  verticalGroupByFields(first: 20) {{ nodes {{ {FIELD_SELECTION} }} }}
                }}
              }}
            }}
          }}
        }}
        "#
    );
    let meta = match graphql(
        &meta_query,
        &[
            ("owner", Scalar::Str(owner.clone())),
            ("number", Scalar::Int(project_number)),
        ],
    )
    .await
    {
        Ok(data) => data,
        Err(err) => return err.envelope(),
    };
    let project = meta.get(node).and_then(|n| n.get("projectV2"));
    let Some(project) = project.filter(|p| !p.is_null()) else {
        return ProjectError::new("not_found", "Project not found.").envelope();
    };

    let views = nodes_map(project.get("views"), map_view);
    let Some(selected_view) = select_view(
        &views,
        view_id.as_deref(),
        view_number,
        view_name.as_deref(),
    ) else {
        return ProjectError::new("not_found", "This project has no views.").envelope();
    };

    // 2) Items, paginated. Try with parent/issueType; on the field-availability
    //    error, retry without them and flag parentFieldDropped.
    let (rows, total_count, parent_dropped, too_large) =
        match fetch_rows(node, &owner, project_number, true).await {
            Ok(result) => result,
            Err(err) if err.kind == "unknown" && mentions_optional_field(&err.message) => {
                match fetch_rows(node, &owner, project_number, false).await {
                    Ok((rows, total, _, too_large)) => (rows, total, true, too_large),
                    Err(err) => return err.envelope(),
                }
            }
            Err(err) => return err.envelope(),
        };

    if too_large {
        let mut envelope = ProjectError::new(
            "too_large",
            format!(
                "This view has {total_count} items — too many to load here. Open it on github.com."
            ),
        )
        .envelope();
        envelope["totalCount"] = json!(total_count);
        return envelope;
    }

    json!({
        "ok": true,
        "data": {
            "project": {
                "id": str_at(project, "id"),
                "owner": owner,
                "ownerType": if node == "organization" { "organization" } else { "user" },
                "number": project_number,
                "title": str_at(project, "title"),
                "url": str_at(project, "url"),
            },
            "selectedView": selected_view,
            "rows": rows,
            "totalCount": total_count,
            "parentFieldDropped": parent_dropped,
        }
    })
}

// True when a GraphQL validation error is about the optional Issue fields we
// retry without (parent / issueType / sub-issues).
fn mentions_optional_field(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("parent")
        || lower.contains("issuetype")
        || lower.contains("sub_issue")
        || lower.contains("subissue")
}

// Page through ProjectV2.items. Returns (rows, totalCount, parentDropped=false,
// tooLarge). parentDropped is decided by the caller (it owns the retry).
async fn fetch_rows(
    node: &str,
    owner: &str,
    project_number: i64,
    with_parent: bool,
) -> Result<(Vec<Value>, i64, bool, bool), ProjectError> {
    let content = content_selection(with_parent);
    let items_query = format!(
        r#"
        query($owner: String!, $number: Int!, $cursor: String) {{
          {node}(login: $owner) {{
            projectV2(number: $number) {{
              items(first: {PAGE_SIZE}, after: $cursor) {{
                totalCount
                pageInfo {{ hasNextPage endCursor }}
                nodes {{
                  id type updatedAt
                  fieldValues(first: 50) {{ nodes {{ {FIELD_VALUE_SELECTION} }} }}
                  content {{ {content} }}
                }}
              }}
            }}
          }}
        }}
        "#
    );

    let mut rows: Vec<Value> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut total_count: i64 = 0;

    loop {
        let mut vars = vec![
            ("owner", Scalar::Str(owner.to_string())),
            ("number", Scalar::Int(project_number)),
        ];
        if let Some(c) = &cursor {
            vars.push(("cursor", Scalar::Str(c.clone())));
        }
        let data = graphql(&items_query, &vars).await?;
        let items = data
            .get(node)
            .and_then(|n| n.get("projectV2"))
            .and_then(|p| p.get("items"));
        let Some(items) = items else {
            return Err(ProjectError::new("not_found", "Project not found."));
        };
        total_count = items
            .get("totalCount")
            .and_then(Value::as_i64)
            .unwrap_or(total_count);

        for node_item in items
            .get("nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rows.push(map_row(node_item, rows.len()));
            if rows.len() >= MAX_ITEMS {
                // Past the cap: signal too_large with the real total.
                return Ok((rows, total_count, false, true));
            }
        }

        let page_info = items.get("pageInfo");
        let has_next = page_info
            .and_then(|p| p.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_next {
            break;
        }
        cursor = page_info
            .and_then(|p| p.get("endCursor"))
            .and_then(Value::as_str)
            .map(String::from);
        if cursor.is_none() {
            break;
        }
    }

    Ok((rows, total_count, false, false))
}

// ─── Field mutations ─────────────────────────────────────────────────────

// Build the `ProjectV2FieldValue` input for one renderer mutation value (the
// `{ kind, ... }` union in shared/github-project-types.ts): the variable
// declarations to append to the mutation signature, the `value:` fragment, and
// the extra vars to bind. Every string travels as a GraphQL variable — the only
// thing ever spliced into the query text is a Rust-formatted f64 (JSON numbers
// can't be NaN/Inf), so nothing user-controlled reaches the query string.
fn field_mutation_value(
    value: &Value,
) -> Result<(String, String, Vec<(&'static str, Scalar)>), ProjectError> {
    let take = |key: &'static str| -> Result<String, ProjectError> {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ProjectError::new(
                    "unknown",
                    format!("Malformed field value: missing `{key}`."),
                )
            })
    };
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    match kind {
        "single-select" => Ok((
            ", $optionId: String!".to_string(),
            "{ singleSelectOptionId: $optionId }".to_string(),
            vec![("optionId", Scalar::Str(take("optionId")?))],
        )),
        "iteration" => Ok((
            ", $iterationId: String!".to_string(),
            "{ iterationId: $iterationId }".to_string(),
            vec![("iterationId", Scalar::Str(take("iterationId")?))],
        )),
        "text" => Ok((
            ", $text: String!".to_string(),
            "{ text: $text }".to_string(),
            vec![("text", Scalar::Str(take("text")?))],
        )),
        "date" => Ok((
            ", $date: Date!".to_string(),
            "{ date: $date }".to_string(),
            vec![("date", Scalar::Str(take("date")?))],
        )),
        "number" => {
            let n = value.get("number").and_then(Value::as_f64).ok_or_else(|| {
                ProjectError::new("unknown", "Malformed field value: missing `number`.")
            })?;
            Ok((String::new(), format!("{{ number: {n} }}"), Vec::new()))
        }
        other => Err(ProjectError::new(
            "unknown",
            format!("Unsupported field value kind `{other}`."),
        )),
    }
}

// Push one field edit (a board drag writes the view's group-by field; table
// cells edit any supported kind). Requires the `project` write scope; without
// it the INSUFFICIENT_SCOPES error classifies as scope_missing and the
// renderer toasts GitHub's message, which names the missing scope.
#[tauri::command]
pub async fn gh_update_project_item_field(
    project_id: String,
    item_id: String,
    field_id: String,
    value: Value,
) -> Value {
    let (extra_decl, fragment, extra_vars) = match field_mutation_value(&value) {
        Ok(parts) => parts,
        Err(err) => return err.envelope(),
    };
    let query = format!(
        "mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!{extra_decl}) {{ \
           updateProjectV2ItemFieldValue(input: {{ projectId: $projectId, itemId: $itemId, fieldId: $fieldId, value: {fragment} }}) \
           {{ projectV2Item {{ id }} }} }}"
    );
    let mut vars = vec![
        ("projectId", Scalar::Str(project_id)),
        ("itemId", Scalar::Str(item_id)),
        ("fieldId", Scalar::Str(field_id)),
    ];
    vars.extend(extra_vars);
    match graphql(&query, &vars).await {
        Ok(_) => json!({ "ok": true }),
        Err(err) => err.envelope(),
    }
}

#[tauri::command]
pub async fn gh_clear_project_item_field(
    project_id: String,
    item_id: String,
    field_id: String,
) -> Value {
    let query = "mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!) { \
        clearProjectV2ItemFieldValue(input: { projectId: $projectId, itemId: $itemId, fieldId: $fieldId }) \
        { projectV2Item { id } } }";
    let vars = [
        ("projectId", Scalar::Str(project_id)),
        ("itemId", Scalar::Str(item_id)),
        ("fieldId", Scalar::Str(field_id)),
    ];
    match graphql(query, &vars).await {
        Ok(_) => json!({ "ok": true }),
        Err(err) => err.envelope(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_insufficient_scopes_as_scope_missing() {
        let errors = json!([{
            "type": "INSUFFICIENT_SCOPES",
            "message": "Your token has not been granted the required scopes to execute this query. The 'number' field requires one of the following scopes: ['read:project']"
        }]);
        let err = classify_graphql_errors(errors.as_array().unwrap());
        assert_eq!(err.kind, "scope_missing");
    }

    #[test]
    fn classifies_not_found() {
        let errors =
            json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a ProjectV2" }]);
        assert_eq!(
            classify_graphql_errors(errors.as_array().unwrap()).kind,
            "not_found"
        );
    }

    #[test]
    fn classifies_scope_message_without_type() {
        let errors =
            json!([{ "message": "requires one of the following scopes: ['read:project']" }]);
        assert_eq!(
            classify_graphql_errors(errors.as_array().unwrap()).kind,
            "scope_missing"
        );
    }

    #[test]
    fn stderr_scope_classification() {
        assert_eq!(
            classify_stderr("error: your token has not been granted the required scopes").kind,
            "scope_missing"
        );
        assert_eq!(
            classify_stderr("gh auth login required").kind,
            "auth_required"
        );
    }

    #[test]
    fn parses_org_project_url_with_view() {
        let parsed = parse_project_ref("https://github.com/orgs/acme/projects/7/views/3").unwrap();
        assert_eq!(
            parsed,
            (Some("organization"), "acme".to_string(), 7, Some(3))
        );
    }

    #[test]
    fn parses_user_project_url() {
        let parsed = parse_project_ref("https://github.com/users/mateo/projects/2").unwrap();
        assert_eq!(parsed, (Some("user"), "mateo".to_string(), 2, None));
    }

    #[test]
    fn parses_owner_number_shorthand_with_unknown_type() {
        let parsed = parse_project_ref("acme/12").unwrap();
        assert_eq!(parsed, (None, "acme".to_string(), 12, None));
    }

    #[test]
    fn rejects_unparseable_ref() {
        assert!(parse_project_ref("not a ref").is_none());
        assert!(parse_project_ref("acme/notanumber").is_none());
    }

    #[test]
    fn maps_single_select_field_with_options() {
        let node = json!({
            "__typename": "ProjectV2SingleSelectField",
            "id": "F1", "name": "Status", "dataType": "SINGLE_SELECT",
            "options": [{ "id": "o1", "name": "Todo", "color": "GRAY" }]
        });
        let mapped = map_field(&node);
        assert_eq!(mapped["kind"], "single-select");
        assert_eq!(mapped["options"][0]["name"], "Todo");
    }

    #[test]
    fn maps_iteration_field_marks_completed() {
        let node = json!({
            "__typename": "ProjectV2IterationField",
            "id": "F2", "name": "Sprint", "dataType": "ITERATION",
            "configuration": {
                "iterations": [{ "id": "i1", "title": "S1", "startDate": "2026-06-01", "duration": 14 }],
                "completedIterations": [{ "id": "i0", "title": "S0", "startDate": "2026-05-18", "duration": 14 }]
            }
        });
        let mapped = map_field(&node);
        assert_eq!(mapped["kind"], "iteration");
        assert_eq!(mapped["iterations"][0]["completed"], false);
        assert_eq!(mapped["iterations"][1]["completed"], true);
    }

    #[test]
    fn maps_plain_field() {
        let node = json!({ "__typename": "ProjectV2Field", "id": "F3", "name": "Title", "dataType": "TITLE" });
        let mapped = map_field(&node);
        assert_eq!(mapped["kind"], "field");
        assert_eq!(mapped["dataType"], "TITLE");
    }

    #[test]
    fn map_view_carries_board_column_field() {
        // A Board view's columns arrive in `verticalGroupByFields` (usually
        // Status); `groupByFields` holds only the optional swimlane grouping.
        let node = json!({
            "id": "V1", "number": 1, "name": "Backlog", "layout": "BOARD_LAYOUT",
            "filter": null,
            "fields": { "nodes": [] },
            "groupByFields": { "nodes": [] },
            "verticalGroupByFields": { "nodes": [{
                "__typename": "ProjectV2SingleSelectField",
                "id": "F1", "name": "Status", "dataType": "SINGLE_SELECT",
                "options": [{ "id": "o1", "name": "Todo", "color": "GRAY" }]
            }] }
        });
        let mapped = map_view(&node);
        assert_eq!(mapped["groupByFields"], json!([]));
        assert_eq!(mapped["verticalGroupByFields"][0]["id"], "F1");
        assert_eq!(mapped["verticalGroupByFields"][0]["kind"], "single-select");
    }

    #[test]
    fn maps_known_field_values_and_drops_unknown() {
        let single = json!({
            "__typename": "ProjectV2ItemFieldSingleSelectValue",
            "optionId": "o1", "name": "Todo", "color": "GRAY",
            "field": { "id": "F1" }
        });
        let (fid, value) = map_field_value(&single).unwrap();
        assert_eq!(fid, "F1");
        assert_eq!(value["kind"], "single-select");
        assert_eq!(value["optionId"], "o1");

        // A value type outside the renderer's union is dropped (empty cell).
        let milestone = json!({
            "__typename": "ProjectV2ItemFieldMilestoneValue",
            "field": { "id": "F9" }
        });
        assert!(map_field_value(&milestone).is_none());
    }

    #[test]
    fn maps_row_with_content_and_field_values() {
        let item = json!({
            "id": "I1",
            "type": "ISSUE",
            "updatedAt": "2026-06-09T00:00:00Z",
            "fieldValues": { "nodes": [
                { "__typename": "ProjectV2ItemFieldTextValue", "text": "hi", "field": { "id": "F0" } }
            ]},
            "content": {
                "__typename": "Issue",
                "number": 42, "title": "Fix bug", "url": "https://x", "state": "OPEN",
                "stateReason": null,
                "repository": { "nameWithOwner": "me/repo" },
                "assignees": { "nodes": [{ "login": "me", "name": "Me", "avatarUrl": "a" }] },
                "labels": { "nodes": [{ "name": "bug", "color": "f00" }] },
                "parent": { "number": 1, "title": "Epic", "url": "https://e" },
                "issueType": { "id": "T1", "name": "Bug", "color": "red", "description": null }
            }
        });
        let row = map_row(&item, 3);
        assert_eq!(row["itemType"], "ISSUE");
        assert_eq!(row["position"], 3);
        assert_eq!(row["content"]["number"], 42);
        assert_eq!(row["content"]["repository"], "me/repo");
        assert_eq!(row["content"]["parentIssue"]["number"], 1);
        assert_eq!(row["content"]["issueType"]["name"], "Bug");
        assert_eq!(row["content"]["isDraft"], Value::Null);
        assert_eq!(row["fieldValuesByFieldId"]["F0"]["text"], "hi");
    }

    #[test]
    fn maps_pr_draft_flag() {
        let item = json!({
            "id": "I2", "type": "PULL_REQUEST", "updatedAt": "2026-06-09T00:00:00Z",
            "fieldValues": { "nodes": [] },
            "content": { "__typename": "PullRequest", "number": 7, "title": "WIP", "isDraft": true }
        });
        let row = map_row(&item, 0);
        assert_eq!(row["content"]["isDraft"], true);
    }

    #[test]
    fn select_view_honors_precedence() {
        let views = vec![
            json!({ "id": "v1", "number": 1, "name": "Board", "layout": "BOARD_LAYOUT" }),
            json!({ "id": "v2", "number": 2, "name": "Table", "layout": "TABLE_LAYOUT" }),
        ];
        // Explicit id wins.
        assert_eq!(
            select_view(&views, Some("v1"), None, None).unwrap()["id"],
            "v1"
        );
        // Number next.
        assert_eq!(
            select_view(&views, None, Some(2), None).unwrap()["number"],
            2
        );
        // Name next.
        assert_eq!(
            select_view(&views, None, None, Some("Board")).unwrap()["name"],
            "Board"
        );
        // Fallback prefers the first TABLE_LAYOUT.
        assert_eq!(select_view(&views, None, None, None).unwrap()["id"], "v2");
    }

    #[test]
    fn select_view_falls_back_to_first_when_no_table() {
        let views =
            vec![json!({ "id": "v1", "number": 1, "name": "Board", "layout": "BOARD_LAYOUT" })];
        assert_eq!(select_view(&views, None, None, None).unwrap()["id"], "v1");
        assert!(select_view(&[], None, None, None).is_none());
    }

    #[test]
    fn optional_field_error_detection() {
        assert!(mentions_optional_field(
            "Field 'parent' doesn't exist on type 'Issue'"
        ));
        assert!(mentions_optional_field("Field 'issueType' doesn't exist"));
        assert!(!mentions_optional_field("Some unrelated error"));
    }

    #[test]
    fn field_mutation_value_binds_strings_as_variables() {
        // The board drop path: single-select (Status columns) and iteration.
        let (decl, fragment, vars) =
            field_mutation_value(&json!({ "kind": "single-select", "optionId": "o1" })).unwrap();
        assert_eq!(decl, ", $optionId: String!");
        assert_eq!(fragment, "{ singleSelectOptionId: $optionId }");
        assert!(matches!(&vars[..], [("optionId", Scalar::Str(s))] if s == "o1"));

        let (decl, fragment, vars) =
            field_mutation_value(&json!({ "kind": "iteration", "iterationId": "it9" })).unwrap();
        assert_eq!(decl, ", $iterationId: String!");
        assert_eq!(fragment, "{ iterationId: $iterationId }");
        assert!(matches!(&vars[..], [("iterationId", Scalar::Str(s))] if s == "it9"));

        let (_, fragment, vars) =
            field_mutation_value(&json!({ "kind": "text", "text": "hello" })).unwrap();
        assert_eq!(fragment, "{ text: $text }");
        assert!(matches!(&vars[..], [("text", Scalar::Str(s))] if s == "hello"));

        let (decl, fragment, _) =
            field_mutation_value(&json!({ "kind": "date", "date": "2026-07-12" })).unwrap();
        assert_eq!(decl, ", $date: Date!");
        assert_eq!(fragment, "{ date: $date }");
    }

    #[test]
    fn field_mutation_value_embeds_only_the_number_literal() {
        let (decl, fragment, vars) =
            field_mutation_value(&json!({ "kind": "number", "number": 3.5 })).unwrap();
        assert_eq!(decl, "");
        assert_eq!(fragment, "{ number: 3.5 }");
        assert!(vars.is_empty());
    }

    #[test]
    fn field_mutation_value_rejects_malformed_and_unknown_kinds() {
        // Missing payload key for the declared kind.
        let err = field_mutation_value(&json!({ "kind": "single-select" })).unwrap_err();
        assert_eq!(err.kind, "unknown");
        assert!(err.message.contains("optionId"));
        // Empty string payloads are as unusable as missing ones.
        assert!(field_mutation_value(&json!({ "kind": "text", "text": "" })).is_err());
        // A kind outside the renderer union must not build a mutation.
        let err = field_mutation_value(&json!({ "kind": "milestone" })).unwrap_err();
        assert!(err.message.contains("milestone"));
        assert!(field_mutation_value(&json!({})).is_err());
    }
}
