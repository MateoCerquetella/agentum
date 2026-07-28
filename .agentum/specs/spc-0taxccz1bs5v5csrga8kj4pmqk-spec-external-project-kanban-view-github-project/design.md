# Architecture: 017a — GitHub Project (v2) read-kanban

> First buildable slice of spec **017** (external-project kanban). Scope:
> **GitHub only, read-only.** Render a GitHub **Project v2** as a kanban with its
> own columns. Linear = 017b; drag-to-write = 017c.

## Core decision: a project board is a LIVE VIEW, not a sync

016/#58 *persist* tracker **issues** into `board_items` (the flat todo/doing/done
board). A **project board** is different: it has its own arbitrary columns and is
the team's live source of truth. So 017a **fetches-and-renders live** — it does
**not** copy the project's columns/cards into `board_items`. Only the *binding*
(which project to show) persists. This keeps 017a small and avoids a second
reconcile engine.

Tradeoff: live-fetch means a network round-trip per view (no offline/local
cache) — accepted for v1; a cache is a later concern. Chosen over persisting
project items because that would duplicate 016's sync machinery for a view.

## Data / persistence

- **Binding: reuse 016's `board_tracker_bindings`** — no new migration. Store
  `provider = "github_project"`, `project = "<owner>/<number>"` (the project's
  URL coordinates). `create_tracker_binding` / `list_tracker_bindings` /
  `delete_tracker_binding` already work generically on `(provider, project)`.
- **No `board_items` writes** in 017a (live view). The project's `projectV2`
  node id is resolved at query time from `owner/number`.

## Server (agentum-server)

GitHub Projects v2 is **GraphQL** (`POST https://api.github.com/graphql`);
`forge.rs` is REST-only, so this is net-new transport. Mirror `linear.rs`'s
existing `graphql()` helper shape.

- **`forge.rs` (or a small `github_graphql` fn)**: `pub(crate) async fn
  github_graphql(token, query, variables) -> Result<Value>` — reuse
  `token_for(ForgeKind::Github)`; auth `Authorization: Bearer`, `Content-Type:
  application/json`, manual body (no reqwest `json` feature, matching `forge_send`).
- **Project fetch** (new module `routes/board_projects.rs` or in `board_sync.rs`):
  - Query `projectV2(number:)` under `organization(login:)`, falling back to
    `user(login:)` (org-vs-user is a common gotcha) → discover the single-select
    **Status** field (its options, in order = the **columns**) + the project's
    **items** (each item's Status option = its column; content → `{title, url}`).
  - Pure parsers (unit-tested): `parse_project_columns(value)`,
    `parse_project_items(value)` → a `ProjectBoard { columns: [{id,name}], cards:
    [{id,title,url,column_id}] }`.
- **Route**: `GET /api/board/projects/board?binding_id=<id>` (auth-gated) →
  resolve binding → `github_graphql` fetch → return `ProjectBoard`. **Fail loud**:
  a GraphQL error / missing `read:project` scope / project-not-found → a clear
  non-2xx with the message (no empty/silent board).

## Desktop (agentum-desktop/ui)

- **`runtime/board-projects-client.ts`** (mirror `board-client.ts`): `getProjectBoard(bindingId)` + `TS` types for `ProjectBoard`.
- **Dynamic-column kanban**: today's `components/tasks/TaskKanbanBoard.tsx` /
  `components/board/BoardPage.tsx` render **fixed** todo/doing/done. Add a view
  (`components/board/ProjectKanbanBoard.tsx`) — or generalize `TaskKanbanBoard`
  to accept a `columns` prop — that renders the **project's** columns in order,
  cards under each, each card deep-linking to GitHub. Bind-project input + a
  "Refresh" action. Wire into `BoardPage`/nav (a "Project board" view).

## Boundaries (what 017a does NOT touch)
- No `board_items` / no 016 reconcile / no `/api/board/sync` changes.
- No write-back (read-only) — drag-to-move is 017c.
- No Linear (017b). No GitLab. No custom fields beyond Status/title (017d).

## Risks → mitigations
- **`read:project` scope** — classic PATs lack it; the GraphQL call 401/403s.
  → explicit error surfaced ("token needs Projects scope"); document it.
- **org vs user projects** — try `organization(login:)`, fall back to
  `user(login:)`; surface "project not found" if neither.
- **Status-field discovery** — a project may name its board field something other
  than "Status". v1: use the first `ProjectV2SingleSelectField` (or one named
  "Status"); configurable later. Documented assumption.
- **Item pagination** — `items(first: 100)`; if more, paginate or cap+`log`.
  v1: cap at 100 + note (no silent truncation).
- **Dynamic-column UI** — the existing kanban assumes 3 fixed columns; render
  from `columns` instead. Keep the change additive (new view), don't break the
  internal board.

## Build order (017a)
1. `github_graphql` helper + token reuse.
2. `projectV2` query consts + pure `parse_project_columns`/`parse_project_items` (+ unit tests on a sample response).
3. `GET /api/board/projects/board` route (resolve binding → fetch → `ProjectBoard`; fail-loud).
4. `board-projects-client.ts`.
5. `ProjectKanbanBoard` view (dynamic columns) + bind/refresh + nav wiring.
6. Verify: `cargo test -p agentum-server --lib` (parsers + route) green; `bun run build` green.

## Acceptance test plan (maps to spec AC)
- **Bind persists**: `create_tracker_binding("github_project", "o/5")` → list → present (reuse 016 store test).
- **Own columns, ordered**: `parse_project_columns` on a sample `projectV2` response → the Status options in order.
- **Cards under columns + deep-link**: `parse_project_items` → each card's `column_id` + `url`.
- **No dup on refresh**: live fetch is stateless → re-fetch yields the same shape (no `board_items` rows created).
- **Fail loud**: a GraphQL `errors` response / missing scope → route returns non-2xx with the message (unit test on the parse/route path).

## YAGNI check
No project-item persistence, no cache, no custom-field model, no write-back, no
column CRUD — all deferred. 017a is: one GraphQL fetch + one route + one binding
(reused) + one dynamic-column view.
