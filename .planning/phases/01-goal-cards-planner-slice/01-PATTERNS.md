# Phase 01: Goal → Cards (planner slice) - Pattern Map

**Mapped:** 2026-05-21
**Files analyzed:** 23 new/modified files
**Analogs found:** 21 / 23 (2 partial; see "No Analog Found")

## File Classification

| New / Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/agentum-store/migrations/0015_orchestrator.sql` | migration | schema | `crates/agentum-store/migrations/0014_board_column_rules.sql` | exact |
| `crates/agentum-core/src/lib.rs` (extend `BoardItem`, `Session`) | type extension | n/a | `agentum_core::BoardItem.lbl` / existing optional-field pattern | exact |
| `crates/agentum-core/src/lib.rs` (new `BoardLink`, `LinkKind`) | type | n/a | `agentum_core::ClaimRequest` + `Status` enum | exact |
| `crates/agentum-store/src/lib.rs` (`add_board_link`, `list_children_of_goal`, `delete_board_link`, `list_board_links_for_goal`) | store method | CRUD | `Store::create_board_item` + `Store::upsert_board_column_rule` | exact |
| `crates/agentum-store/src/lib.rs` (extend `create_board_item` / `patch_board_item` to carry `parent_goal_id`) | store method (extend) | CRUD | itself — same function, same shape | exact |
| `crates/agentum-store/src/lib.rs` (extend `create_session` / patch to carry `card_id`) | store method (extend) | CRUD | `Store::create_session` / `update_status_and_target` | exact |
| `crates/agentum-store/src/paths.rs::planner_config_path()` | path helper | filesystem | `paths::auth_token_path()`, `paths::tls_dir()` | exact |
| `crates/agentum-server/src/routes/board_goals.rs` (new — `POST /api/board/goals`) | route handler | request-response | `routes/board_rules.rs::upsert` + `routes/sessions.rs::start` | role-match |
| `crates/agentum-server/src/routes/board_links.rs` (new — `POST/GET/DELETE /api/board/links`) | route handler | CRUD | `routes/board_rules.rs` (full file) | exact |
| `crates/agentum-server/src/routes/board.rs` (extend `create`/`patch` ctx with `parent_goal_id`) | route handler (extend) | request-response | itself | exact |
| `crates/agentum-server/src/routes/mod.rs` (register `board_goals`, `board_links`) | route registry | n/a | itself (flat `pub mod` list) | exact |
| `crates/agentum-server/src/lib.rs::router()` (merge new routers) | composition | n/a | itself | exact |
| `crates/agentum-server/src/planner.rs` (new — `planner.toml` reader + bundled-prompt resolution) | config loader | file-I/O | `crates/agentum-server/src/rules.rs` + TUI's `terminal/profiles.rs` | partial |
| `crates/agentum-watchdog/src/lib.rs` (extend `watch_session` status-emit path + new goal-status recomputer) | watchdog hook | event-driven | `watch_session` itself (status-emit branches) | exact |
| `crates/agentum/src/commands/board/mod.rs` (new — `BoardCmd` dispatch) | CLI subcommand parent | request-response | `crates/agentum/src/commands/auth.rs::run` + `commands/profiles.rs::run` | exact |
| `crates/agentum/src/commands/board/add_goal.rs` (new) | CLI subcommand | request-response | `crates/agentum/src/commands/send.rs` + `commands/auth.rs::run_setup_wizard` | partial |
| `crates/agentum/src/commands/board/add_card.rs` (new) | CLI subcommand | request-response | same as `add_goal.rs` | partial |
| `crates/agentum/src/commands/board/planner_prompt.md` (new — bundled default) | embedded asset | n/a | no in-repo `include_str!` analog | **none** |
| `crates/agentum/src/cli.rs` (register `BoardCmd`) | CLI registration | n/a | existing `Cmd::Auth`, `Cmd::Profiles` dispatch | exact |
| `crates/agentum/src/commands/terminal/app.rs::Overlay::Goal` (+ `GoalForm`) | TUI overlay | event-driven | `Overlay::NewSession(Box<NewSessionForm>)` | exact |
| `crates/agentum/src/commands/terminal/ui.rs` (render Overlay::Goal + parent-cue chip) | TUI render | n/a | existing `Overlay::NewSession` render | exact |
| `dashboard/src/lib/components/GoalComposer.svelte` (new) | dashboard component | request-response | `dashboard/src/lib/components/BoardItemDialog.svelte` (submit path) | role-match |
| `dashboard/src/lib/stores/board.ts::submitGoal(text)` (new action) | store action | request-response | `dashboard/src/lib/stores/board.ts::loadBoard` / `moveLocal` / `patchStatusWithSnapBack` | exact |
| `dashboard/src/routes/board/+page.svelte` (parent-cue chip + filter pill) | dashboard render (extend) | n/a | itself (existing `.col-h`, `.tk-foot` rendering) | exact |
| `dashboard/src/lib/themes/_design.css` (`.lbl.goal`, `.lbl.parent-cue`) | CSS extension | n/a | existing `.lbl.bug`, `.lbl.feat` rules | exact |

---

## Pattern Assignments

### `crates/agentum-store/migrations/0015_orchestrator.sql` (migration, schema)

**Analog:** `crates/agentum-store/migrations/0014_board_column_rules.sql`

**Comment-style template** (lines 1–22, full file):
```sql
-- Per-server override of the compile-time required-field matrix from
-- slice 1 (`agentum-core::board_schema::required_fields_for`). A row
-- here replaces the const default for that column; an absent row means
-- "use the const" for `todo`/`doing`/`done` and "passthrough" for any
-- other column.
--
-- `required_fields` is a JSON array of wire-vocabulary strings (e.g.
-- `["title","lbl","workdir"]`) — same identifiers
-- `RequiredField::as_missing_key` produces. JSON-blob over a normalised
-- two-column table because rules are read as a complete set per column;
-- no slice 2 query looks up by individual field. Denormalisation costs
-- nothing and keeps the upsert a single statement.
--
-- `updated_at` is the only audit affordance in slice 2 — deliberately
-- not exposed via `GET /api/board/rules` to keep the wire shape minimal
-- until a dedicated audit endpoint lands.

CREATE TABLE board_column_rules (
    column_name      TEXT PRIMARY KEY,
    required_fields  TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
```

**New code reuses:** the comment-first-then-DDL layout. Must add prose at the top of `0015_orchestrator.sql` explaining (a) why `parent_goal_id` is nullable + additive (backwards-compat), (b) why `board_links` is its own table not an inline JSON column on `board_items` (PROJECT.md constraint: "sub-10ms graph walk; no per-edge SQL query" — Phase 3 expects to query edges directly), (c) why `card_id` is nullable on `sessions`. DDL must be `ALTER TABLE board_items ADD COLUMN parent_goal_id INTEGER`, `ALTER TABLE sessions ADD COLUMN card_id INTEGER`, plus a new `CREATE TABLE board_links (from_card_id INTEGER NOT NULL, to_card_id INTEGER NOT NULL, kind TEXT NOT NULL, ...)`.

---

### `crates/agentum-core/src/lib.rs` — extend `BoardItem` + `Session`, add `BoardLink`

**Analog (in same file):** `BoardItem.lbl`, `Session.tokens` — see lines 285–315 and 87–113

**Optional-field pattern** (`crates/agentum-core/src/lib.rs:285-315`):
```rust
/// Ticket type — colors the foot label pill.
/// One of: `bug` | `feat` | `chore` | `spike`. Free-form String so
/// users can introduce custom labels without a schema change.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub lbl: Option<String>,
/// Tool ecosystem — colors the foot dot.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub tool: Option<String>,
/// agentum session id that this ticket is being worked in. Set when
/// the user spawns a session from the ticket; nullable so cards
/// without an active session can still render the Start affordance.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub session_id: Option<String>,
```

**New code reuses:** add `pub parent_goal_id: Option<i64>` with the **same `#[serde(default, skip_serializing_if = "Option::is_none")]`** attribute. Add a sibling field on `NewBoardItem` (same pattern: `#[serde(default)] pub parent_goal_id: Option<i64>`). Add `pub parent_goal_id: Option<Option<i64>>` on `BoardPatch` with `#[serde(default, deserialize_with = "deserialize_optional_field")]` — this is the double-Option pattern explained at lines 360–368 that distinguishes "field omitted" from "explicitly null", critical for letting a PATCH detach a child from its goal.

For `Session.card_id`: same shape, optional `i64`, `#[serde(default, skip_serializing_if = "Option::is_none")]`.

**New `BoardLink` + `LinkKind` enum** — mirror the existing tight enum pattern from `Status` (`lib.rs:28-46`) and the typed struct pattern from `BoardComment` (`lib.rs:380-388`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    ParentOf,
    Blocks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardLink {
    pub from_card_id: i64,
    pub to_card_id: i64,
    pub kind: LinkKind,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
```
The `LinkKind` enum must implement `as_str` + `FromStr` for the SQLite text column, copying the body of `Status::as_str` and the `FromStr` impl at `lib.rs:38-65`.

---

### `crates/agentum-store/src/lib.rs` — new link methods + extend board/session methods

**Analog A (single-row INSERT):** `Store::create_board_item` at lines 309–362
**Analog B (upsert + SELECT pair):** `Store::upsert_board_column_rule` at lines 657–680 and `get_board_column_rule` at 622–632

**INSERT pattern** (`crates/agentum-store/src/lib.rs:309-344`):
```rust
pub async fn create_board_item(&self, new: NewBoardItem) -> Result<BoardItem> {
    let now = OffsetDateTime::now_utc();
    let now_s = now.format(&Rfc3339)?;
    let status = new.status.unwrap_or_else(|| "todo".to_string());
    let priority = new.priority.unwrap_or(0);

    let mut tx = self.pool.begin().await?;
    let result = sqlx::query(
        "INSERT INTO board_items (key, title, body, status, lbl, tool, workdir, model, session_id, priority, created_at, updated_at)
         VALUES ('', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new.title)
    .bind(&new.body)
    ...
    .execute(&mut *tx)
    .await?;
    let id = result.last_insert_rowid();
    let key = format!("AG-{id}");
    sqlx::query("UPDATE board_items SET key = ? WHERE id = ?")
        .bind(&key)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
```

**Upsert pattern (board_column_rules)** (`crates/agentum-store/src/lib.rs:657-680`):
```rust
pub async fn upsert_board_column_rule(
    &self,
    column: &str,
    fields: &[RequiredField],
) -> Result<()> {
    let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let wire: Vec<&'static str> = fields.iter().map(|f| f.as_missing_key()).collect();
    let json = serde_json::to_string(&wire)?;
    sqlx::query(
        "INSERT INTO board_column_rules (column_name, required_fields, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(column_name) DO UPDATE SET
            required_fields = excluded.required_fields,
            updated_at = excluded.updated_at",
    )
    .bind(column)
    .bind(&json)
    .bind(&now_s)
    .execute(&self.pool)
    .await?;
    Ok(())
}
```

**list-with-filter pattern** (`crates/agentum-store/src/lib.rs:638-651`):
```rust
pub async fn list_board_column_rules(
    &self,
) -> Result<std::collections::BTreeMap<String, Vec<RequiredField>>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT column_name, required_fields FROM board_column_rules")
            .fetch_all(&self.pool)
            .await?;
    let mut out = std::collections::BTreeMap::new();
    for (col, json) in rows {
        let parsed = parse_rule_json(&col, &json)?;
        out.insert(col, parsed);
    }
    Ok(out)
}
```

**Delete-returning-bool pattern** (`crates/agentum-store/src/lib.rs:684-691`):
```rust
pub async fn delete_board_column_rule(&self, column: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM board_column_rules WHERE column_name = ?")
        .bind(column)
        .execute(&self.pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}
```

**New code reuses:** raw-SQL `sqlx::query` / `sqlx::query_as` — **never `query!` macros** (per RESEARCH.md / CLAUDE.md / `0014` precedent: no live DB required at build time). RFC3339 timestamps via `OffsetDateTime::now_utc().format(&Rfc3339)?`. `rows_affected() == 0` → `StoreError::NotFound`. Multi-row transactions use `self.pool.begin().await?` + `tx.commit().await?` exactly as in `create_board_item`.

For `add_board_link(from: i64, to: i64, kind: LinkKind)`: single `INSERT INTO board_links (from_card_id, to_card_id, kind, created_at) VALUES (?, ?, ?, ?)`. For `list_children_of_goal(goal_id: i64) -> Vec<BoardItem>`: `SELECT * FROM board_items WHERE parent_goal_id = ? ORDER BY priority ASC, created_at ASC` — same `ORDER BY` shape as `list_board_items` at line 369.

For the watchdog's `max(child statuses)` recompute: add a focused method `Store::max_child_status_rank(goal_id: i64) -> Result<Option<i32>>` that runs `SELECT MAX(CASE status WHEN 'todo' THEN 0 WHEN 'doing' THEN 1 WHEN 'done' THEN 2 ELSE -1 END) FROM board_items WHERE parent_goal_id = ?`. Returns `None` for the empty-children case (max-of-empty-set per D-03).

---

### `crates/agentum-store/src/paths.rs::planner_config_path()`

**Analog (in same file):** `auth_token_path()` line 45–47, `tls_dir()` line 49–51

**Excerpt** (`crates/agentum-store/src/paths.rs:25-51`):
```rust
pub fn config_dir() -> Result<PathBuf, PathError> {
    Ok(dirs()?.config_dir().to_path_buf())
}
...
pub fn auth_token_path() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("auth_token"))
}

pub fn tls_dir() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("tls"))
}
```

**New code reuses:** one-line public fn returning `Result<PathBuf, PathError>` that joins to `config_dir()?`:
```rust
pub fn planner_config_path() -> Result<PathBuf, PathError> {
    Ok(config_dir()?.join("planner.toml"))
}
```
**Critical:** sibling to `profiles.toml` + `credentials.toml` (both already under `config_dir()`), per CONTEXT D-12.

---

### `crates/agentum-server/src/routes/board_links.rs` (new — CRUD on `board_links`)

**Analog:** `crates/agentum-server/src/routes/board_rules.rs` (entire file, 1–103 for the handler shape)

**Imports + module doc** (`routes/board_rules.rs:1-22`):
```rust
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
```

**Router pattern** (`routes/board_rules.rs:24-28`):
```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/rules", get(list))
        .route("/api/board/rules/{column}", put(upsert).delete(delete))
}
```

**Handler signature + event-emit shape** (`routes/board_rules.rs:44-87`):
```rust
async fn upsert(
    State(state): State<AppState>,
    Path(column): Path<String>,
    Json(body): Json<UpsertBody>,
) -> Result<StatusCode, ApiError> {
    // Parse field names through the typed enum so the store gets a
    // validated input.
    let mut parsed: Vec<RequiredField> = Vec::with_capacity(body.required_fields.len());
    for name in &body.required_fields {
        match RequiredField::from_missing_key(name) {
            Some(f) => parsed.push(f),
            None => {
                return Err(ApiError::BadRequest(format!("unknown field: {name}")));
            }
        }
    }
    ...
    state.store.upsert_board_column_rule(&column, &parsed).await?;
    let _ = state.bus.send(Event::new("board.rules.updated").with_payload(json!({...})));
    Ok(StatusCode::OK)
}
```

**New code reuses:**
- Module-doc top line naming the URL prefix (`//! /api/board/links — ...`).
- Imports in canonical order (`agentum_core`, `axum`, `serde`, `serde_json`, then `crate::`).
- `pub fn router() -> Router<AppState>` exactly as above; routes literal before dynamic: `/api/board/links` (POST/GET) + `/api/board/links/{from}/{to}/{kind}` (DELETE).
- Extractor order `State<AppState>, Path<…>, Json<…>` (per RESEARCH.md / CONTEXT canonical).
- Returns `Result<Json<T>, ApiError>` / `Result<(StatusCode, Json<T>), ApiError>` / `Result<StatusCode, ApiError>` for 200/201/204.
- After every successful write emit a dotted-kind event via `state.bus.send(Event::new("board.link.created").with_payload(json!(...)))`; the dashboard's `/api/events` WS picks it up free.
- Match `ApiError::Conflict` for `(from, to, kind)` UNIQUE violation; `ApiError::BadRequest` for unknown kind string; `ApiError::NotFound` when `from` or `to` board_item doesn't exist.

---

### `crates/agentum-server/src/routes/board_goals.rs` (new — `POST /api/board/goals` atomic create-goal + spawn-planner)

**Primary analog (atomic multi-step write + spawn):** `routes/sessions.rs::start` at lines 220–275
**Secondary analog (handler shape):** `routes/board_rules.rs::upsert` (already excerpted above)

**Spawn-a-session pattern** (`crates/agentum-server/src/routes/sessions.rs:220-275`):
```rust
async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let target = agentum_tmux::target_for(&session.name);

    let already = agentum_tmux::has_session(&target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if matches!(session.status, Status::Running) && already {
        return Ok(Json(session));
    }
    ...
    let workdir = PathBuf::from(&session.workdir);
    if !workdir.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }

    let adapter = agentum_executor::adapter_for(&session.tool);
    let launch = adapter.launch(&session);

    agentum_tmux::new_session(&target, &workdir, &launch.argv, &launch.env)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let log = paths::pane_log(&session.id.to_string())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    state
        .store
        .update_status_and_target(id, Status::Running, Some(&target))
        .await?;
    Ok(Json(load(&state, id).await?))
}
```

**Gate-aware POST pattern** (`routes/board.rs::create` at lines 137-168):
```rust
async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewBoardItem>,
) -> Result<(StatusCode, Json<BoardItem>), ApiError> {
    let target_status = payload.status.as_deref().unwrap_or("todo");
    let mut ctx = TransitionCtx {
        title: Some(payload.title.as_str()),
        lbl: payload.lbl.as_deref(),
        ...
    };
    enforce_transition(&state.store, &state.bus, None, target_status, &mut ctx).await?;

    let item = state.store.create_board_item(payload).await?;
    let _ = state.bus.send(
        Event::new("board.created")
            .with_payload(json!({"id": item.id, "key": item.key, "title": item.title})),
    );
    Ok((StatusCode::CREATED, Json(item)))
}
```

**New code reuses:** the goal-submission handler must:
1. Build a `NewBoardItem { title, lbl: Some("goal"), status: Some("todo"), ..Default::default() }` and call `enforce_transition` exactly like `board::create`. This is the single biggest correctness point — per CONTEXT `<canonical_refs>`, if the user has tightened `todo`'s column rule via Slice 2, the goal POST must surface that 400 via the existing `Custom(BAD_REQUEST, {"missing": [...], "status": "todo"})` envelope so `GoalComposer.svelte` can render the localized error from copy contract.
2. Insert the board_item via `state.store.create_board_item(...)`. Capture `goal.id`.
3. Read `planner.toml` via the new `crate::planner` module; default `tool = "claude"`.
4. Construct `NewSession { name: format!("planner-{}", goal.key.to_lowercase()), workdir: <inferred-or-provided>, tool: planner_tool, model: None, flags: vec![] }`, call `state.store.create_session(new)` *with* `card_id = Some(goal.id)` set (extended signature).
5. Spawn the session inline — copy lines 247–273 of `sessions::start` verbatim: `adapter_for(tool).launch(&session)` → `agentum_tmux::new_session` → `pipe_pane(log)` → `update_status_and_target(Status::Running, Some(target))`. **Critical:** on tmux failure, follow the existing pattern of `kill_session(&target)` before returning the error so we don't leak orphaned tmux sessions.
6. Inject the bundled planner prompt as the session's first `send_keys` after the pane is up.
7. Emit `goal.created` + `goal.planner.spawned` events on the bus.
8. Return `(StatusCode::CREATED, Json(goal_item))`.

**Failure-rollback invariant:** if the planner spawn fails, the handler should *not* delete the goal card (per D-07: user can still inspect via `tail`). Surface the spawn failure as a side-channel event (`goal.planner.spawn_failed`) and return the goal with a 201; the dashboard renders the warning chip from the event.

---

### `crates/agentum-server/src/routes/board.rs` — extend `create` + `patch` to carry `parent_goal_id`

**Analog:** same file, `create` at lines 137–168 and `patch` at 182–255 (already excerpted above)

**New code reuses:** thread `parent_goal_id: Option<i64>` through `NewBoardItem` → `create_board_item` → `BoardItem` (already a single-line addition once the struct field exists). For `BoardPatch`, follow the existing double-Option pattern: a PATCH that sets `parent_goal_id: Option<Option<i64>>` to `Some(None)` *clears* the link; `Some(Some(id))` *changes* it; `None` leaves the field alone. Do NOT add `parent_goal_id` to the `TransitionCtx` — it's not a column-rule field, just a relational pointer.

---

### `crates/agentum-server/src/planner.rs` (new — `planner.toml` reader + bundled default)

**Partial analog A:** `crates/agentum-server/src/rules.rs` (the composition layer that touches the store and falls back to a compile-time default)
**Partial analog B:** `crates/agentum/src/commands/terminal/profiles.rs` (the on-disk TOML reader pattern, `load()` / `Profiles::path()`)

**Excerpt — rules.rs fallback pattern** (`crates/agentum-server/src/rules.rs:25-34`):
```rust
pub async fn resolve_required_fields(
    store: &Store,
    status: &str,
) -> Result<Cow<'static, [RequiredField]>, ApiError> {
    if let Some(override_) = store.get_board_column_rule(status).await? {
        Ok(Cow::Owned(override_))
    } else {
        Ok(Cow::Borrowed(required_fields_for(status)))
    }
}
```

**New code reuses:** same "DB-or-disk override → bundled-const fallback" shape, returning a `Cow<'static, str>` for the prompt so the const path is alloc-free. Read order per D-12: `prompt_file` → `prompt` → bundled default. Return shape:
```rust
pub struct PlannerConfig {
    pub tool: String,           // default "claude"
    pub prompt: Cow<'static, str>,
}

pub async fn load_planner_config() -> Result<PlannerConfig, ApiError> {
    let path = agentum_store::paths::planner_config_path()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Missing file => bundled default with tool="claude".
    if !path.exists() {
        return Ok(PlannerConfig {
            tool: "claude".into(),
            prompt: Cow::Borrowed(BUNDLED_PROMPT),
        });
    }
    // Read + parse `planner.toml`; resolution order: prompt_file -> prompt -> bundled.
    // (file-I/O via tokio::fs to keep the handler async-clean)
    ...
}

const BUNDLED_PROMPT: &str = include_str!("../../agentum/src/commands/board/planner_prompt.md");
```

**Critical:** `include_str!` is **not** used anywhere else in this codebase today (verified via `grep -rn 'include_str!' crates/`). This is a new convention. Anchor the bundled path relative to the *consumer* crate (`agentum-server`) using a `../../` path that points into `crates/agentum/src/commands/board/planner_prompt.md` so the same markdown is read by both the CLI's help text (if needed) and the daemon's planner-spawn first-message. If cross-crate `include_str!` causes build-tree fragility, alternative: vendor the prompt into `crates/agentum-server/src/planner_prompt.md` and have the CLI read it via the daemon (less duplication, looser coupling — let the planner step decide).

---

### `crates/agentum-watchdog/src/lib.rs` — extend `watch_session` for goal-status recompute

**Analog (same file):** `watch_session` activity-classification + status-emit branches at lines 191–264, the `emit` helper at line 579, and the entire reconciliation philosophy ("the watchdog is the only writer of auto-goal-status — never a client PATCH" per CONTEXT D-03).

**Excerpt — status update + event emit** (`crates/agentum-watchdog/src/lib.rs:248-265`):
```rust
// Crash signatures first — exiting wins over compacting.
if let Some(sig) = crash_sigs.iter().find(|s| pane.contains(*s)) {
    if intentionally_stopped(&store, sess.id).await {
        return;
    }
    tracing::warn!(name = %sess.name, signature = sig, "crash signature matched");
    let _ = store
        .update_status_and_target(sess.id, Status::Crashed, None)
        .await;
    let ev = Event::new("session.crashed")
        .with_session(sess.id, &sess.name)
        .with_payload(serde_json::json!({"signature": sig}));
    let _ = emit(&bus, &store, ev).await;
    return;
}
```

**New code reuses:** the watchdog's recompute is **not** in `watch_session` (a per-session loop). It belongs in a new sibling subscriber to the `board.updated` event stream — or, simpler and matching the existing pattern: extend the reconcile loop's per-tick work. The clean architectural fit:

1. **Add a new tokio task** spawned alongside `Watchdog::run` that subscribes to `state.bus` and filters for `board.updated` / `board.deleted` / `board.created` events whose payload references a row with `parent_goal_id IS NOT NULL`.
2. **On match**, call the new `store.max_child_status_rank(parent_goal_id)`, compare to the parent's current status rank, and if different, call `store.patch_board_item(parent_goal_id, BoardPatch { status: Some(<computed>), ..Default::default() })`.
3. **Emit** `goal.status.changed` with payload `{goal_id, from, to}` so the dashboard updates the goal card's column placement.

Critically, the recompute path must NOT route through `enforce_transition` (the watchdog bypasses the gate by writing through `Store::patch_board_item` directly, mirroring the existing back-door tests in `routes/board.rs::tests` at line 657 that use `store.patch_board_item` to simulate legacy rows). This avoids the goal needing `workdir`/`tool` when its computed status crosses into `doing`.

The "first child arrives" auto-stop for the planner (CONTEXT D-07) hooks in here too: when the recomputer sees a `board.created` event with `parent_goal_id` set, fire `goal.planner.first_child`, then call `routes::sessions::stop` (or equivalent in-process) on the planner session bound via `session.card_id = goal.id`.

**Constraint that lives in the comments around this code:** "No recursion — goals don't have parents in v1, so the recompute stops at depth 1" (CONTEXT line 372). Encode that as an explicit comment + a debug-assert: `debug_assert!(goal.parent_goal_id.is_none(), "v1 invariant: goals don't have parents")`.

---

### `crates/agentum/src/commands/board/mod.rs` + `add_goal.rs` + `add_card.rs` (new CLI surface)

**Analog A (subcommand dispatch parent):** `crates/agentum/src/commands/auth.rs::run` at lines 7–55, `commands/profiles.rs::run` at lines 11–24
**Analog B (HTTP-client subcommand body — partial):** `commands/send.rs` (sends to local tmux pane, not HTTP) and `commands/profiles.rs::add` (reads/writes local config, not HTTP)
**Analog C (credentials file path):** `commands/terminal/trust.rs::creds_path()` at lines 252–255

**Subcommand-dispatch shell** (`commands/auth.rs:7-21`):
```rust
pub async fn run(action: AuthCmd) -> Result<()> {
    let (store, _) = super::open_store().await?;

    match action {
        AuthCmd::List => {
            let users = store.list_users().await?;
            if users.is_empty() {
                println!("(no users — run `agentum auth setup` to create the first admin)");
                return Ok(());
            }
            for u in users {
                println!("{:>4}  {:<24}  {}", u.id, u.username, u.created_at);
            }
            Ok(())
        }
        AuthCmd::Add { username, password } => { ... }
        ...
    }
}
```

**Credentials path** (`commands/terminal/trust.rs:252-255`):
```rust
fn creds_path() -> Result<PathBuf> {
    let dir = agentum_store::paths::config_dir().map_err(|e| anyhow!("resolve config dir: {e}"))?;
    Ok(dir.join("credentials.toml"))
}
```

**Anyhow error envelope + exit code** (`commands/send.rs:4-22`):
```rust
pub async fn run(name: String, text: String) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let Some(session) = store.get_session_by_name(&name).await? else {
        eprintln!("no session named {name}");
        std::process::exit(3);
    };
    ...
    if !matches!(session.status, Status::Running) || !agentum_tmux::has_session(&target).await? {
        bail!("session {name} is not running");
    }

    agentum_tmux::send_keys(&target, &text, true).await?;
    println!("sent        {name}  \"{text}\"");
    Ok(())
}
```

**New code reuses:**
- `mod.rs` defines `pub async fn run(action: BoardCmd) -> Result<()>` and `match`es on `BoardCmd::AddGoal{..}` / `BoardCmd::AddCard{..}`.
- `cli.rs` adds `pub enum BoardCmd { AddGoal { ... }, AddCard { ... } }` with clap `#[command(subcommand)]` glue exactly like `AuthCmd` at line 357.
- Each subcommand calls a tiny `agentum-board-cli` HTTP client (new module) that:
  1. Resolves the local-loopback base URL from `~/.config/agentum/profiles.toml` (default profile, or the `local` profile per CONTEXT D-08).
  2. Loads the bearer token from `~/.config/agentum/credentials.toml` via the existing `terminal::trust` helpers (the spec calls these out by name — verify the exact function names during planning; likely `load_credentials` / `creds_for_url` already exist in `terminal/trust.rs`).
  3. POSTs to `/api/board/goals` (for `add-goal`) or `/api/board` + `/api/board/links` (for `add-card`).
  4. Prints the assigned `AG-{id}` key to stdout, one per line (the planner agent parses these from its scrollback).
  5. On unknown sibling-key for `--blocks <key>`, exit non-zero with message `unknown sibling key: <key>` (per CONTEXT D-06).
  6. Missing `credentials.toml` → bail with a one-line hint, exactly the pattern in `commands/send.rs:7-9`: `eprintln!("..."); std::process::exit(N);` — exit 4 reserved for "credentials missing" per current code's exit-code spread.

---

### `crates/agentum/src/commands/board/planner_prompt.md` (bundled default planner prompt)

**Analog:** **none in this codebase.** `grep -rn 'include_str!' crates/` returns zero hits. This is a new convention.

**Risk note:** the planner must budget extra discovery time here. Three options under consideration:
1. `include_str!` from the agentum binary crate (closest to D-13's wording — "baked into the binary via `include_str!`").
2. `include_str!` from `agentum-server` (so the daemon owns it, since the daemon is what spawns the planner).
3. Ship the prompt as a real file on disk at first-boot (copied to `$XDG_CONFIG_HOME/agentum/planner.toml.example` or similar).

Recommended: option 2 — the daemon owns the bundled string, and the CLI `agentum board add-*` shims read whatever the daemon dispatches over the wire. This avoids duplicating the prompt across crates AND avoids the cross-crate `include_str!` path-fragility risk. Let the planning step lock the choice.

**Prompt content** (per CONTEXT D-13): "names the `agentum board add-goal` / `add-card` CLI surface, explains `--key` / `--blocks` semantics with a worked example, and ends with 'emit `<DONE>` when finished'." The content itself is design work — not a pattern question — and gets drafted in the planning step.

---

### `crates/agentum/src/commands/terminal/app.rs::Overlay::Goal` + `GoalForm`

**Analog (same file):** `Overlay::NewSession(Box<NewSessionForm>)` at line 180, `NewSessionForm` struct at lines 360–385, `NewSessionForm::with_profile` constructor at lines 453–473.

**Overlay enum variant** (`crates/agentum/src/commands/terminal/app.rs:158-189`):
```rust
#[derive(Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    ...
    Rename(RenameState),
    /// New-session form (n key on the tree).
    NewSession(Box<NewSessionForm>),
    /// Generic confirmation prompt for destructive session actions.
    Confirm(PendingAction),
    /// Server switcher. ...
    Profiles(ProfilesOverlay),
}
```

**Form struct** (`crates/agentum/src/commands/terminal/app.rs:359-385`):
```rust
#[derive(Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub field: NewSessionField,
    pub profile: String,
    pub name: String,
    pub tool: String,
    pub model: String,
    pub workdir: String,
    pub args: String,
    pub up_after: bool,
    pub yolo: bool,
    pub error: Option<String>,
    pub submitting: bool,
    pub picker: Option<DirPickerState>,
    pub tool_picker: Option<ToolPickerState>,
}
```

**Constructor** (`crates/agentum/src/commands/terminal/app.rs:453-473`):
```rust
impl NewSessionForm {
    pub fn with_profile(default_profile: String, default_workdir: String) -> Self {
        Self {
            field: NewSessionField::Profile,
            profile: default_profile,
            name: String::new(),
            tool: "claude".into(),
            ...
            error: None,
            submitting: false,
            picker: None,
            tool_picker: None,
        }
    }
    ...
}
```

**New code reuses:**
- `Overlay::Goal(Box<GoalForm>)` variant alongside `NewSession`. `Box` because `GoalForm` will hold a String buffer that can grow; matches the enum-size discipline.
- `GoalForm { text: String, submitting: bool, error: Option<String>, profile: String }` — single-field form (multiline textarea), no fields cycle (so no `field: GoalField`).
- Lifecycle dispatch via the existing `apply_event` + `handle_key` paths at the lines that handle `Overlay::NewSession` (4760–5081). The `G` keybinding lands in the board-view key dispatch (per UI-SPEC interaction contract — *not* lowercase `g`).
- Submit path mirrors the `submit_new_session` (around line 4760): take `Overlay::Goal`, set `submitting = true`, dispatch to a `client.submit_goal(text)` call (new API client method), on success close the overlay; on failure populate `form.error` and reopen with the error visible.
- Status bar copy from UI-SPEC: while submitting, the status bar shows `GOAL · planning…`. This hooks into the existing status-bar render path in `ui.rs`.
- Help overlay must document the new `G` binding under the Board section, alongside the existing entries (see `Overlay::Help` rendering in `ui.rs`).

---

### `dashboard/src/lib/components/GoalComposer.svelte`

**Analog A (submit path / error handling / rejection parsing):** `dashboard/src/lib/components/BoardItemDialog.svelte` (submit fn at lines 278–354, `parseRejectionFromMessage` at lines 356–370)
**Analog B (store action shape):** existing `loadBoard` / `moveLocal` / `patchStatusWithSnapBack` in `dashboard/src/lib/stores/board.ts`

**Submit + gate-rejection pattern** (`dashboard/src/lib/components/BoardItemDialog.svelte:278-354`, condensed):
```ts
async function submit(e: SubmitEvent) {
    e.preventDefault();
    const t = title.trim();
    if (!t) {
      error = 'title is required';
      return;
    }
    submitting = true;
    error = null;
    try {
      ...
      const created = await api.createBoardItemOn(targetProfileId, payload);
      onCreated?.(targetProfileId, created);
      onClose();
    } catch (err) {
      // Server gate rejection? The payload carries {missing, status}.
      if (err instanceof ApiError && err.status === 400) {
        const parsed = parseRejectionFromMessage(err.message);
        if (parsed) {
          rejectedFields = new Set(parsed.missing);
          const labels = parsed.missing.map(requiredFieldLabel).join(', ');
          error = `move to ${parsed.status} needs: ${labels}`;
        } else {
          error = err.message;
        }
      } else {
        error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      submitting = false;
    }
  }

  function parseRejectionFromMessage(message: string): ReturnType<typeof parseGateRejection> {
    const idx = message.indexOf('{');
    if (idx < 0) return null;
    try {
      return parseGateRejection(JSON.parse(message.slice(idx)));
    } catch {
      return null;
    }
  }
```

**Store-action shape** (`dashboard/src/lib/stores/board.ts:15-24`):
```ts
export async function loadBoard() {
  board.update((s) => ({ ...s, loading: true, error: null }));
  try {
    const data = await api.listBoard();
    board.set({ loading: false, error: null, data });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    board.update((s) => ({ ...s, loading: false, error: msg }));
  }
}
```

**New code reuses:**
- Props shape uses Svelte 5 runes (`$state`, `$props`, `$derived`, `$effect`) per project conventions — see `BoardItemDialog.svelte:53-66`.
- Submit must call a new `submitGoal(text)` store action that wraps `api.createGoal(text)` (a thin POST to `/api/board/goals`). The store action follows the `loadBoard` shape: dispatch, await, update store on success, surface error on failure.
- The error-rendering envelope (red `--crash` left border + `role="alert"` + `aria-live="polite"`) must follow the UI-SPEC's Interaction Contract verbatim.
- Per UI-SPEC, **no toast on success** — the new goal card arriving via the existing `/api/events` WS *is* the feedback. The store's existing event-bridge will pick up the `board.created` + `goal.created` events; no special wiring needed.
- The 400 `{missing, status}` rejection path reuses `parseGateRejection` + `requiredFieldLabel` from `$lib/board-schema` (per UI-SPEC Component Inventory and existing `BoardItemDialog.svelte` import at line 6).
- "Planning…" submit state per UI-SPEC: button disabled, label changes, no spinner glyph.

---

### `dashboard/src/lib/stores/board.ts::submitGoal(text)` (new action)

**Analog (same file):** `loadBoard()` at lines 15–24, `patchStatusWithSnapBack()` at lines 80–111

Already excerpted above. **New code reuses:** identical try/catch/store-update shape. The action calls `api.createGoal(text)` (a new thin wrapper in `dashboard/src/lib/api.ts`) which POSTs to `/api/board/goals` with body `{ title: text }`. On 400 with `{missing, status}` body, the calling component handles the rejection — the store action just re-throws so the composer can render the inline error per UI-SPEC.

---

### `dashboard/src/routes/board/+page.svelte` (parent-cue chip + filter pill)

**Analog (same file):** existing `.col-h` and `.tk-foot` rendering inside the file. Use the project pattern — Svelte 5 runes for component-local `boardFilter` state, no URL persistence (per UI-SPEC line 188).

**New code reuses:** the file already imports board items; add (a) a one-liner inside the `.tk-foot .lbls` cluster that renders `<button class="lbl parent-cue">↳ AG-{parent_goal_id}</button>` whenever the item has `parent_goal_id` set, and (b) the column-header filter pill near `.col-h .add`, scoped to local state. Click handlers route to existing `openCard(parentGoalId)` (the same opener `BoardItemDialog.svelte` reads from).

---

### `dashboard/src/lib/themes/_design.css` (`.lbl.goal`, `.lbl.parent-cue`)

**Analog (same file):** existing `.lbl.feat`, `.lbl.bug`, `.lbl.chore`, `.lbl.spike` rules.

**New code reuses:** add `.ticket .tk-foot .lbl.goal { color: var(--cta); border-color: rgba(243, 100, 88, 0.4); }` and `.ticket .tk-foot .lbl.parent-cue { color: var(--fg-3); background: var(--bg-2); border: 1px solid var(--border-2); border-radius: var(--radius-sm); }` plus hover `.lbl.parent-cue:hover { color: var(--link); border-color: var(--link); }`. Per UI-SPEC Color section + the file's existing pattern, **do not introduce new tokens** — reuse `--cta`, `--link`, `--fg-3`, `--bg-2`, `--border-2`, `--radius-sm`.

---

## Shared Patterns

### Authentication (HTTP)
**Source:** `crates/agentum-server/src/auth.rs::require_token` middleware, mounted in `crates/agentum-server/src/lib.rs::router()`
**Apply to:** All new route files (`board_goals.rs`, `board_links.rs`).
**Pattern:** Route modules don't enforce auth themselves — the middleware is applied at the top-level router merge. Just register the new `pub fn router() -> Router<AppState>` and `.merge(routes::board_goals::router())` in `lib.rs::router()`; auth is automatic. Public-path allow-list lives in `auth.rs::is_public`; do **not** add the new routes to it.

### Error Envelope (HTTP)
**Source:** `crates/agentum-server/src/error.rs::ApiError` + `routes/board.rs::enforce_transition`
**Apply to:** All new route handlers.

**Pattern excerpt** (`routes/board.rs:124-133`):
```rust
let _ = bus.send(Event::new("board.transition.rejected").with_payload(json!({...})));
Err(ApiError::Custom(
    StatusCode::BAD_REQUEST,
    json!({ "missing": missing, "status": target_status }),
))
```

**New code reuses:** default `ApiError::BadRequest("msg")` produces `{"error": "msg"}`; gate failures use `ApiError::Custom(BAD_REQUEST, json!({ "missing": [...], "status": "..." }))` so the dashboard's `parseGateRejection` keeps working unmodified. Goal-create that fails the column-rule gate **must** preserve this exact envelope.

### Event Bus + WS Fan-out
**Source:** `AppState::bus` (`crates/agentum-server/src/lib.rs:69`) + every existing route's `state.bus.send(Event::new("..."))`
**Apply to:** All new write endpoints + the watchdog's new recomputer.
**Pattern:** After every successful write, `let _ = state.bus.send(Event::new("kind.dot.path").with_payload(json!({...})));`. Dashboard's `/api/events` WS picks up everything; no additional wiring needed. New event kinds for Phase 1: `goal.created`, `goal.planner.spawned`, `goal.planner.first_child`, `goal.planner.spawn_failed`, `goal.status.changed`, `board.link.created`, `board.link.deleted`.

### Tracing (logging)
**Source:** `tracing::info!` / `warn!` / `error!` everywhere; never `eprintln!` in TUI paths (`CLAUDE.md` warning, reinforced in CONTEXT line 341–342)
**Apply to:** all new code paths.
**Pattern:** Daemon paths use `tracing::warn!(error = ?e, "context")`. TUI paths use `tracing::info!` only — the TUI's tracing layer writes to `$XDG_CACHE_HOME/agentum/tui.log` so anything on stderr would scramble the alt-screen.

### Embedded SPA Rebuild Rhythm
**Source:** `CLAUDE.md` "Critical: rebuild rhythm" + UI-SPEC §"Embedded SPA Rebuild Rhythm"
**Apply to:** every plan task that touches `dashboard/src/`.
**Pattern:** the plan **must** include `npm run build --prefix dashboard && cargo build --release` as a follow-up step before the daemon serves the new bundle. Without it, the running daemon serves the OLD bundle (the `rust-embed` `dashboard/build/` tree is baked at compile time).

### sqlx Convention
**Source:** RESEARCH.md + every method in `crates/agentum-store/src/lib.rs`
**Apply to:** all new store methods.
**Pattern:** **Runtime queries only** — `sqlx::query`, `sqlx::query_as`. **No compile-time `query!` / `query_as!` macros.** Multi-line SQL uses `r#"…"#` raw strings (CONTEXT-pinned). UNIQUE-violation detection via `if let Err(sqlx::Error::Database(db)) = &res { if db.is_unique_violation() { return Err(StoreError::AlreadyExists(...)); } }` exactly as in `create_session` at lines 115–119. Use `OffsetDateTime::now_utc().format(&Rfc3339)?` for every timestamp; bind via `.bind(...)`; never string-format values into SQL.

---

## No Analog Found

| File | Role | Reason / Risk |
|------|------|---------------|
| `crates/agentum/src/commands/board/planner_prompt.md` (bundled prompt resource) | embedded asset | `grep -rn 'include_str!' crates/` returns **zero hits**. No existing convention for compile-time-embedded text content. Planner needs to choose between (a) cross-crate `include_str!` from agentum-server into agentum's tree, (b) vendoring the prompt inside agentum-server, (c) shipping as a `planner.toml.example` on first boot. **Risk: build-tree fragility.** Recommended: option (b) — daemon owns the prompt, CLI shims never read the file. |
| `crates/agentum-server/src/planner.rs` (planner config + bundled-prompt loader) | config loader | Partial analogs only (`rules.rs` for the const-fallback shape, `terminal/profiles.rs` for the on-disk TOML reader). No existing daemon-side TOML reader that combines both. **Risk: medium.** Plan should reference both partial analogs explicitly and call out the synthesis (DB-or-file → bundled-const fallback, using `Cow<'static, str>` to keep the default path alloc-free). |

---

## Metadata

**Analog search scope:**
- `crates/agentum-core/src/lib.rs` (BoardItem, Session, ClaimRequest, BoardComment, Status — full file)
- `crates/agentum-store/src/lib.rs` (create_board_item, list_board_items, patch_board_item, get/upsert/list/delete_board_column_rule, paths)
- `crates/agentum-store/migrations/0014_board_column_rules.sql`
- `crates/agentum-store/src/paths.rs` (full file)
- `crates/agentum-server/src/routes/board.rs` (create, patch, enforce_transition, full test module)
- `crates/agentum-server/src/routes/board_rules.rs` (full file including all tests)
- `crates/agentum-server/src/routes/sessions.rs` (create, start, stop, kill, patch_session)
- `crates/agentum-server/src/routes/mod.rs`
- `crates/agentum-server/src/rules.rs` (full file)
- `crates/agentum-executor/src/lib.rs` (full file)
- `crates/agentum-watchdog/src/lib.rs` (Watchdog::run, reconcile, watch_session, emit)
- `crates/agentum/src/commands/auth.rs` (run, run_setup_wizard, prompt_password)
- `crates/agentum/src/commands/send.rs` (full file)
- `crates/agentum/src/commands/keys.rs` (full file)
- `crates/agentum/src/commands/profiles.rs` (run, list, add, remove)
- `crates/agentum/src/commands/terminal/app.rs` (Overlay enum, NewSessionForm)
- `crates/agentum/src/commands/terminal/trust.rs::creds_path` and surrounds
- `crates/agentum/src/cli.rs` (Cmd, AuthCmd, ProfilesCmd)
- `dashboard/src/lib/components/BoardItemDialog.svelte` (submit, parseRejectionFromMessage)
- `dashboard/src/lib/stores/board.ts` (full file)

**Files scanned:** ~20 source files, ~3500 lines of Rust + ~500 lines of Svelte/TS read targeted (not whole-file).

**Pattern extraction date:** 2026-05-21

**Cross-cutting principle:** every new file has a direct, in-repo analog except the two flagged in "No Analog Found" — those two are higher-risk and the planner should budget an extra discovery cycle when scoping their tasks.
