# Architecture: 014a — Mapping foundation + GitHub PULL (import)

> First buildable slice of parent spec **014** (two-way board ↔ tracker sync).
> Scope here is **one direction, one provider**: pull a GitHub repo's open
> issues onto the board as cards, idempotently. Push-back is 014b; Linear 014c;
> desktop surface 014d.

## Key decision: reuse `forge.rs`, don't reimplement

`crates/agentum-server/src/routes/forge.rs` already speaks GitHub/GitLab REST
(`reqwest`), stores a token at `<data_dir>/forge.json` (0600), and normalizes
issues. 014a **reuses** its primitives instead of shelling `gh`:

- `ForgeKind`, `Remote { kind, api_base, project }`
- `classify_remote(host, project) -> Option<Remote>` (build a github Remote via
  `classify_remote("github.com", project)`)
- `forge_get(remote, token, url) -> Result<Value>`
- `token_for(kind) -> Result<String>`
- `str_field(v, key)`

→ Make those `pub(crate)` in `forge.rs` (the only change to that file). It is
**not** in the foreign-WIP set, so this is safe.

## Data model (migration `0022_board_external_sync.sql`)

Add to `board_items` (all nullable, `#[sqlx(default)]` like `parent_goal_id`):
- `external_provider TEXT` — `"github"` (later `"gitlab"`/`"linear"`)
- `external_id TEXT` — issue number as string (provider-native id)
- `external_url TEXT` — issue web URL (deep-link)
- `external_synced_at TEXT` — RFC3339 of the last sync that touched the card

New table for the durable binding:
```sql
CREATE TABLE IF NOT EXISTS board_tracker_bindings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    provider    TEXT NOT NULL,
    project     TEXT NOT NULL,           -- "owner/repo"
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS board_tracker_bindings_uniq
    ON board_tracker_bindings(provider, project);
CREATE INDEX IF NOT EXISTS board_items_external_idx
    ON board_items(external_provider, external_id);
```

## Types (`agentum-core/src/lib.rs`)

- Extend **`BoardItem`** with the 4 `external_*` fields (`Option<String>`,
  `#[serde(default, skip_serializing_if = "Option::is_none")]`). Update the
  **two** real constructors (store `create_board_item` Ok-block → all `None`;
  store `try_from` → from row) and the **one** test helper
  (`board_goals.rs:~680 card_with`).
- **Do NOT touch `NewBoardItem`** (≈40 call sites). The sync path uses a
  dedicated store method instead.
- Add **`TrackerBinding { id, provider, project, created_at, updated_at }`**
  (rfc3339 serde on the timestamps, mirroring `BoardItem`).

## Store (`agentum-store/src/lib.rs`)

- `BoardItemRow`: add the 4 `external_*` columns with `#[sqlx(default)]`; map in
  `try_from`.
- `upsert_external_card(provider, external_id, title, body, url, status, now)
  -> Result<(BoardItem, bool /*created*/)>` — match by
  `(external_provider, external_id)`. If found: UPDATE title/body/external_url/
  status/external_synced_at/updated_at. Else: INSERT (same `key='AG-{id}'`
  post-insert dance as `create_board_item`, `lbl="feat"`, `claimed_by=NULL`).
  Returns the row + whether it was created. **Does not** go through
  `create_board_item` (keeps the session dual-write path untouched).
- `list_external_refs(provider) -> Result<Vec<(i64 id, String external_id, String status)>>`
  — reconcile input.
- Bindings: `create_tracker_binding(provider, project)`,
  `list_tracker_bindings()`, `delete_tracker_binding(id)` (INSERT OR REPLACE on
  the unique index for create/update-in-place).
- Tests: binding roundtrip; `upsert_external_card` create-then-update is
  idempotent (same row id, fields refreshed); external fields survive
  `list_board_items`.

## Sync engine + route (`agentum-server/src/routes/board_sync.rs`, NEW)

Pure core (unit-tested, no I/O):
```rust
struct ExternalIssue { external_id: String, title: String, body: Option<String>, url: String, state: String } // state: "open"|"closed"
enum SyncAction { Create { issue: ExternalIssue, status: String },
                  Update { card_id: i64, issue: ExternalIssue, status: String } }
fn reconcile(existing: &[(i64, String /*ext_id*/, String /*status*/)], issues: &[ExternalIssue]) -> Vec<SyncAction>
fn state_to_status(state: &str) -> &str        // "closed"=>"done", else "todo"
fn reconcile_status(local: &str, state: &str) -> String
//   closed                     => "done"
//   open && local == "done"    => "todo"   (reopened upstream)
//   open                       => local     (preserve local todo/doing)
fn parse_github_issues(v: &Value) -> Vec<ExternalIssue>  // skip objects with "pull_request" key
```
Route (auth-gated; reuses forge `pub(crate)` helpers):
- `POST /api/board/bindings  {provider, project}` → 201 `TrackerBinding`
  (provider must be `"github"` in 014a; others → 400 "not yet supported (014c)").
- `GET  /api/board/bindings` → `[TrackerBinding]`
- `DELETE /api/board/bindings/{id}` → 204
- `POST /api/board/sync` (optional `{binding_id}`; else all bindings) → for each
  binding: `classify_remote("github.com", project)` → `token_for(Github)` →
  `forge_get(".../repos/{project}/issues?state=all&per_page=100")` →
  `parse_github_issues` → `reconcile(store.list_external_refs("github"), …)` →
  apply each action via `store.upsert_external_card(…)`. Returns
  `{results:[{provider, project, created, updated}]}` and emits a
  `board.sync.completed` event. **Fail loud** (no token / forge error) — surface
  the error, no silent partial success beyond what was already upserted.
- Tests: `reconcile` (create vs update, status map, reopen, preserve-doing),
  `parse_github_issues` (filters PRs), bindings CRUD smoke. Live network sync is
  **not** unit-tested (runtime / `#[ignore]`), matching the forge + 011 pattern.

## Wiring
- `routes/mod.rs`: `pub mod board_sync;` (file is foreign-modified — add the one
  line surgically).
- `lib.rs`: `.merge(routes::board_sync::router())` next to the other `board_*`
  merges (foreign-modified — surgical).

## Boundaries / non-goals (this slice)
- No push-back (board→tracker) — 014b.
- No Linear / GitLab pull — 014c (GitLab host needs more than `github.com`).
- No desktop UI — 014d.
- No background/auto sync — manual `POST /api/board/sync` only.
- No conflict UI; the `reconcile_status` rule above is the documented v1 policy.

## Verify
`cargo test -p agentum-store -p agentum-server --lib` green; `cargo clippy
-p agentum-server -p agentum-store` clean. **Safety:** create/edit ONLY the
files named here; never `git add -A` or commit (foreign WIP in tree); push is
human-gated.
