# Architecture Notes — Spec 016a (Server-side GitHub PULL + durable binding + migration)

> Scope: **016a ONLY**. Server-side GitHub issue **pull** into the board, a durable
> board↔tracker **binding**, and the **migration** that extends #58's columns.
> **Explicitly excludes** push-back (016b), Linear (016c), and any desktop surface
> (016d). This builds **on top of** #58's shipped one-way client mirror — it does
> not fork it.

This slice ports the *proven* pull/binding/reconcile logic from the closed `feat/014d`
reference branch onto the current `develop` base. It deliberately re-ports logic, not
commits, because `develop` and the reference branch diverge at exactly the three points
that closed PRs #68/#69/#71 (migration, route contract, `linear.rs`). Each is designed
out below.

---

## Components

The build target is a **fresh branch off `origin/develop`** (which carries #58 in
v0.19.0). The reference branch (`feat/014d`, this checkout) has the donor code but
**not** #58 — never merge it; copy the logic.

### Backend crates touched

| Crate | File | NEW / MODIFIED | What |
| --- | --- | --- | --- |
| `agentum-store` | `migrations/00NN_board_external_two_way.sql` | **NEW** | The next-free migration (NN ≥ `0023`; **developer confirms next-free at build time** — `0021` and `0022` are both taken on develop). **Extends** #58's `board_items` with `external_id TEXT` + `external_synced_at TEXT`; adds the `board_tracker_bindings` table; adds the reconcile lookup index `board_items(external_provider, external_id)`. Does **not** redefine #58's `external_url`/`external_provider` columns or its partial-unique index on `external_url`. |
| `agentum-store` | `src/lib.rs` | **MODIFIED** | Add `external_id` + `external_synced_at` to the `BoardItemRow` struct + its `TryFrom<BoardItemRow> for BoardItem`. Add store helpers (port from reference): `upsert_external_card`, `list_external_refs`, `create_tracker_binding`, `list_tracker_bindings`, `delete_tracker_binding`. (Reference also has `set_card_external_ref` — that is a **push-back** helper; omit in 016a to keep the slice minimal → 016b.) |
| `agentum-core` | `src/lib.rs` | **MODIFIED** | Add `external_id: Option<String>` + `external_synced_at: Option<String>` fields to `BoardItem` (#58 already added `external_provider` + `external_url`). Add the `TrackerBinding` struct (port verbatim from reference). |
| `agentum-server` | `src/routes/board_sync.rs` | **NEW** | The pull route module. Port the **pure** sync core from the reference (`ExternalIssue`, `SyncAction`, `state_to_status`, `reconcile_status`, `reconcile`, `parse_github_issues`) and the **bindings CRUD** + a **pull** handler — but on **distinct paths** (see APIs). **Strip** everything push-side and Linear-side (`push_card`, `resolve_push_target`, `status_to_state`, `parse_repo_from_issue_url`, the `linear::*` arms). GitHub pull only. |
| `agentum-server` | `src/lib.rs` | **MODIFIED** | `.merge(routes::board_sync::router())` in the top-level `router()`. (On develop this merge does not yet exist — #58 ships only `board::router()`.) |

### Reuse (no change)

- `agentum-server::routes::forge` — `ForgeKind`, `Remote`, `classify_remote`,
  `forge_get`, `token_for`. The GitHub REST fetch + token store + error→502 mapping
  are already correct and tested. **Do not reimplement HTTP.** `forge_get` already
  surfaces a non-2xx forge response as a `BAD_GATEWAY` `ApiError` before any board
  write — that is the spine of the fails-loud AC.
- `agentum-server::error::ApiError` — `BadRequest` / `NotFound` / `Custom(502)` /
  `Internal` cover every failure path here.
- `AppState.store` + `AppState.bus` — the store handle and the broadcast event bus
  (emit `board.binding.created` / `board.binding.deleted` / `board.sync.completed`).

---

## APIs

The contract is split so #58's path is **byte-for-byte untouched**.

| Method + Path | Owner | Body | Notes |
| --- | --- | --- | --- |
| `POST /api/board/sync` | **#58 (`board.rs`) — UNTOUCHED** | `{ items: [{ external_url, external_provider, title, body, status, lbl }] }` | The shipped **client-supplied batch** mirror. 016a must not register, rename, or alter this. A regression test guards it. |
| `POST /api/board/bindings` | **016a (`board_sync.rs`)** | `{ provider: "github", project: "owner/repo" }` | Create/idempotent-rebind a durable binding. Validates `provider == "github"` and `project` contains `/`. (Reference accepts `linear` too — **reject non-github in 016a**.) → `201 { TrackerBinding }`. |
| `GET /api/board/bindings` | **016a** | — | List bindings. → `200 [TrackerBinding]`. |
| `DELETE /api/board/bindings/{id}` | **016a** | — | Remove a binding. → `204`; `404` if absent. |
| `POST /api/board/bindings/{id}/sync` | **016a — the server pull trigger** | — (id is in the path) | Fetch the bound GitHub repo's issues → upsert as cards, idempotent, matched by external ref. → `200 { provider, project, created, updated }`. `404` if the binding id is unknown; `400` if no token; `502` if GitHub is unreachable/errors — and in every error case **no board rows are written**. |

> **Why `/bindings/{id}/sync` and not the reference's `/api/board/sync {binding_id?}`:**
> the reference put pull on `POST /api/board/sync` with a `{binding_id?}` body — that is
> the *exact* path #58 now owns with an incompatible `{items:[…]}` body. Co-locating
> them is the collision that closed PR #69/#71. Hanging pull off the binding resource
> (`/bindings/{id}/sync`) is REST-clean, takes the target from the path (no body needed),
> and can **never** clash with `/api/board/sync`.

Pure, unit-tested functions (no I/O), ported from the reference's `board_sync.rs` tests:
`state_to_status`, `reconcile_status`, `reconcile`, `parse_github_issues`. These already
have a complete test suite in the reference — port the tests with them.

---

## Data Flow

**Bind (once, durable):**
`POST /api/board/bindings {github, owner/repo}` → `store.create_tracker_binding`
(`ON CONFLICT(provider, project) DO UPDATE updated_at` — idempotent rebind) → row in
`board_tracker_bindings`, persisted across daemon restart → emit `board.binding.created`.

**Sync now (pull):**
`POST /api/board/bindings/{id}/sync` →
1. `store.list_tracker_bindings()` → resolve `{id}`; unknown → `404`, **no writes**.
2. `classify_remote("github.com", project)` + `token_for(Github)` (missing token →
   `400`, **no writes**).
3. `forge_get(GET {api_base}/repos/{project}/issues?state=all&per_page=100)` →
   on any non-2xx or transport error, `forge_get` returns `ApiError` → handler returns
   it, **no writes** (satisfies the stubbed-unreachable AC).
4. `parse_github_issues(json)` → `Vec<ExternalIssue>` (drops PRs via the `pull_request`
   key; skips malformed rows; maps GitHub `state` → `todo`/`done` column).
5. `store.list_external_refs("github")` → `[(card_id, external_id, status)]`.
6. `reconcile(existing, issues)` → `Vec<SyncAction::{Create,Update}>` (pure; matched by
   `external_id`; `reconcile_status` decides the column for updates).
7. For each action → `store.upsert_external_card(provider, external_id, title, body, url,
   status, synced_at)` — insert-or-update by `(external_provider, external_id)`, stamping
   `external_synced_at`. Idempotent: a re-sync of an unchanged issue updates in place,
   never duplicates.
8. Emit `board.sync.completed`; return `{ created, updated }`.

Crucial ordering invariant: **all network I/O (steps 2–4) completes before any store
write (step 7).** A failure short-circuits with `?` before mutation. That is *how* the
"no board changes on failure" AC holds — no transaction rollback gymnastics needed.

---

## Important Decisions

### Key tradeoff — reconcile by `(provider, external_id)`, not `external_url`

**Chosen:** add a stable `external_id` and reconcile on `(external_provider, external_id)`
(the reference's key), rather than reuse #58's `external_url` as the match key.

**Why:** the external **id** (GitHub issue *number*) is the issue's stable identity; the
**url** is a derived, theoretically-mutable label (host/owner/repo renames, GHE base
changes). Two-way sync needs an identity that survives a round-trip and never ping-pongs
(an AC + a named risk). `external_id` gives the upsert a precise, indexable target and
matches the proven, already-tested reference upsert (`upsert_external_card` +
`list_external_refs`). #58's `external_url` column **stays** (still populated on every
upsert for the deep-link and to keep the client mirror's partial-unique index satisfied);
it simply isn't the match key. So this is **extend**, not fork: same table, same `external_url`
semantics for #58's client path, plus the two columns two-way needs.

Rejected — reconcile on `external_url`: it would force `upsert_external_card` to match on a
mutable string, can't represent "same issue, new url", and would diverge from the only
working reconcile code we have. It buys nothing 016a needs.

### Extend #58's columns; do not add a parallel schema

The new migration (`00NN ≥ 0023`) issues `ALTER TABLE board_items ADD COLUMN external_id`
and `ADD COLUMN external_synced_at` — the two #58 omitted — plus the `board_tracker_bindings`
table and the `(external_provider, external_id)` index. It must **not** re-add
`external_url`/`external_provider` (they exist) and must **not** create a parallel
sync table. (The reference's `0022_board_external_sync.sql` added all four columns at once;
on develop two already exist, so only the missing two are added under a new number.)
On develop, `BoardItem`/`BoardItemRow` carry only `external_provider` + `external_url`
today; this slice adds the matching `external_id` + `external_synced_at` fields so the
struct mirrors the extended table.

### Single source of the reconcile/column policy

`reconcile_status` (ported verbatim) is the one place the pull-side column policy lives:
closed upstream → `done`; reopened (was `done`, now open) → take the tracker column;
otherwise preserve the local column (don't yank a manual `todo`→`doing`). Full two-sided
conflict detection (`conflicts[]`) is **016b** — 016a is one direction, so its sync result
is `{created, updated}` and carries no conflicts (a pull can't conflict with itself).

### `linear.rs` is a 016c concern — out of scope here

The reference `board_sync.rs` calls `linear::pull_issues` / `linear::update_issue`; those
arms are **stripped** in the 016a port. The existing `linear.rs` (the SDD→Linear task
sink) is **not touched**. The module-merge collision (#68) is real but lives entirely in
016c. Noted, deferred.

---

## Risks

| Risk | Mitigation (designed in) |
| --- | --- |
| **#58 regression** — the closed-PR cause. If pull touches the shipped `POST /api/board/sync {items}` path it breaks the client mirror. | Pull lives on a **separate** resource route (`/api/board/bindings/{id}/sync`); `board.rs` is **not edited**. An **integration regression test** asserts `POST /api/board/sync {items:[…]}` still returns success unchanged (this is an AC). The 016a router merge only *adds* `board_sync::router()`; it removes nothing. |
| **Migration numbering** — `0021` and `0022` are both taken on develop; reusing a number corrupts the migration ledger (the other closed-PR cause). | Migration is `00NN` where **NN = the next free number ≥ `0023`**, **verified by the developer at build time** (`ls crates/agentum-store/migrations`), not assumed. `sqlx::migrate!("./migrations")` sequences by numeric prefix, so a fresh number is append-only and safe. Never rename or reuse `0022`. |
| **main-/working-tree WIP hazard** — this repo's tree routinely holds foreign agent WIP. | Build on a **fresh branch off `origin/develop`**. Stage only this slice's own hunks (`git add -p` / hash-object + update-index); **never `git add -A`**; never `checkout`/`reset`/`stash` the tree. |
| **Reconcile identity instability / ping-pong** | Durable `external_id` + `external_synced_at` marker + idempotent `upsert_external_card` keyed on `(provider, external_id)`. The ported `reconcile_status` tests prove round-trip stability (open preserves local column; closed→done; reopen→tracker column). |
| **Fails-loud must make zero board changes** (AC). | All forge I/O precedes the first store write; `forge_get`/`token_for` return `ApiError` (→ `400`/`502`) and short-circuit with `?` before step 7. Test with a stubbed-unreachable remote and assert card count + contents are unchanged. |
| **Porting drift** — re-typing donor code can introduce bugs the reference already fixed (PR/malformed-row filtering, empty-body→None, `state=all`). | Port the reference's pure functions **with their existing unit tests** (`parse_github_issues_filters_prs_and_skips_bad_rows`, `reconcile_*`, `state_to_status_*`). Green tests = faithful port. |
| **GitHub pagination (>100 issues)** | Out of scope (reference deferred it). `per_page=100`, single page. Acceptable for 016a; note as a follow-up. Do **not** add pagination machinery now. |

---

## Explicit non-goals (016a)

- **No push-back** (board → tracker). No `push_card`, no `set_card_external_ref` wiring,
  no `status_to_state`. → 016b.
- **No Linear.** No `linear.rs` edits; GitHub provider only. → 016c.
- **No desktop surface.** No edits to `runtime/board-client.ts`, `BoardPage.tsx`,
  `TaskPage.tsx`, or any `ui/` file. Pull is exercised via the HTTP route + tests only.
  → 016d.
- **No GitLab.** Binding creation rejects anything but `github`.
- **No background/periodic auto-sync.** Manual trigger only.
- **No webhooks**, no conflict-resolution `conflicts[]` field (a pull alone can't
  conflict). → 016b.
- **No pagination, no label/assignee/milestone mapping.** Status + title/body + url only.

---

## Build order / task outline (for the Developer)

Small, sequenced, each independently compilable + testable.

1. **Branch.** Create a fresh branch off `origin/develop` (e.g. `feat/016a-board-server-pull`).
   Confirm #58 is present (`git grep upsert_board_item_by_external_url` and the
   `0022_board_external_link.sql` migration both exist). **Do not** branch off this
   `feat/014d` checkout.
2. **Migration.** `ls crates/agentum-store/migrations` → take the next free `00NN`
   (≥ `0023`). Write `00NN_board_external_two_way.sql`: `ALTER TABLE board_items ADD
   COLUMN external_id TEXT;` + `... ADD COLUMN external_synced_at TEXT;`; `CREATE TABLE
   IF NOT EXISTS board_tracker_bindings (id INTEGER PK AUTOINCREMENT, provider TEXT NOT
   NULL, project TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);`
   `CREATE UNIQUE INDEX ... board_tracker_bindings(provider, project);`
   `CREATE INDEX ... board_items(external_provider, external_id);`. **Do not** re-add
   `external_url`/`external_provider`. (Donor: `0022_board_external_sync.sql`, minus the
   two already-present columns.)
3. **Core types.** `agentum-core`: add `external_id` + `external_synced_at` to `BoardItem`;
   add the `TrackerBinding` struct (donor: this checkout's core lib.rs).
4. **Store.** `agentum-store/src/lib.rs`: add the two fields to `BoardItemRow` + its
   `TryFrom`; port `upsert_external_card`, `list_external_refs`, `create_tracker_binding`,
   `list_tracker_bindings`, `delete_tracker_binding` (donor lines ~699–855) **with** their
   store tests (`upsert_external_card_is_idempotent_on_re_sync`, the bindings CRUD test).
   Omit `set_card_external_ref` (016b).
5. **Route — pure core.** New `routes/board_sync.rs`: port `ExternalIssue`, `SyncAction`,
   `state_to_status`, `reconcile_status`, `reconcile`, `parse_github_issues` **and the
   unit tests** verbatim. Compile + `cargo test` the pure layer before wiring I/O.
6. **Route — handlers.** Bindings CRUD (`create_binding` rejecting non-github,
   `list_bindings`, `delete_binding`) + `sync_binding` for `POST /api/board/bindings/{id}/sync`
   (resolve binding by path id → `forge_get` → `parse_github_issues` → `reconcile` →
   `upsert_external_card` loop → `{created,updated}`). Reuse `forge::{classify_remote,
   forge_get, token_for}`. Emit the three bus events.
7. **Wire.** Add `.merge(routes::board_sync::router())` to `lib.rs::router()`.
8. **Regression test (AC).** Integration test: `POST /api/board/sync {items:[…]}` still
   succeeds unchanged against the running router.
9. **Fails-loud test (AC).** Integration test with a stubbed-unreachable GitHub (bad
   token or unreachable base) → `sync` returns non-2xx **and** board card count +
   contents are unchanged.
10. **Verify.** `cargo test -p agentum-core -p agentum-store -p agentum-server --lib`
    (+ the new integration tests) green on macOS. Stage only own hunks; never `git add -A`.

---

### Files (absolute paths)

- Spec: `ai/specs/016-board-server-two-way-sync/spec.md`
- Handoff (PM): `ai/specs/016-board-server-two-way-sync/handoffs/01-pm-to-architect.md`
- Donor route (port FROM, strip push/Linear): `crates/agentum-server/src/routes/board_sync.rs` (this `feat/014d` checkout)
- Donor store helpers: `crates/agentum-store/src/lib.rs` (~lines 691–888, tests ~3061–3185)
- Donor core types: `crates/agentum-core/src/lib.rs` (BoardItem ~541–574)
- Reuse (no change): `crates/agentum-server/src/routes/forge.rs`
- Donor migration (minus 2 present columns): `crates/agentum-store/migrations/0022_board_external_sync.sql`
- Router merge point: `crates/agentum-server/src/lib.rs` (the `router()` fn)
- **Do NOT touch** on develop: `routes/board.rs` (owns `POST /api/board/sync`), `linear.rs`, any `crates/agentum-desktop/ui/**`.
