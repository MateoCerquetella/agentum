# Tasks — Spec 016a (Server-side GitHub PULL + durable binding + migration)

Honest checkbox list against `architecture.md`'s 10-step build order. Checked =
implemented **and** verified (compiles + tests green). Work done on branch
`feat/016a-board-server-pull` (worktree `.claude/worktrees/016a-board-server-pull`),
based on `origin/develop` @ 0ad4d58 (carries #58).

## Build order

- [x] **1. Branch.** Fresh branch off `origin/develop` (NOT `feat/014d`). #58 confirmed
      present: `0022_board_external_link.sql` + `upsert_board_item_by_external_url` both
      exist on the base.
- [x] **2. Migration.** Verified next-free number at build time (`ls migrations` → last is
      `0022_board_external_link.sql`), so **0023** is next-free. Wrote
      `crates/agentum-store/migrations/0023_board_external_two_way.sql`:
      `ALTER TABLE board_items ADD COLUMN external_id TEXT;` +
      `ADD COLUMN external_synced_at TEXT;`; `CREATE TABLE board_tracker_bindings (…)`;
      `CREATE UNIQUE INDEX … board_tracker_bindings(provider, project)`;
      `CREATE INDEX … board_items(external_provider, external_id)`. Did **not** re-add
      `external_url`/`external_provider`; did **not** touch #58's partial-unique index.
- [x] **3. Core types.** `agentum-core`: added `external_id: Option<String>` +
      `external_synced_at: Option<String>` to `BoardItem`; added the `TrackerBinding` struct.
- [x] **4. Store.** `agentum-store/src/lib.rs`: added the 2 fields to `BoardItemRow` + its
      `TryFrom` (and the `create_board_item` native-card initializer); ported
      `upsert_external_card`, `list_external_refs`, `create_tracker_binding`,
      `list_tracker_bindings`, `delete_tracker_binding` **with** their store tests
      (`upsert_external_card_is_idempotent_on_re_sync`, `tracker_binding_roundtrip_and_rebind`).
      **Omitted** `set_card_external_ref` (push-back → 016b).
- [x] **5. Route — pure core.** New `routes/board_sync.rs`: ported `ExternalIssue`,
      `SyncAction`, `state_to_status`, `reconcile_status`, `reconcile`,
      `parse_github_issues` **with** their unit tests (verbatim). **Stripped** all
      push-side (`push_card`, `resolve_push_target`, `status_to_state`,
      `parse_repo_from_issue_url`) and all `linear::*` arms.
- [x] **6. Route — handlers.** Bindings CRUD (`create_binding` rejecting non-github,
      `list_bindings`, `delete_binding`) + `sync_binding` on
      `POST /api/board/bindings/{id}/sync` (resolve binding by path id → `forge_get` →
      `parse_github_issues` → `reconcile` → `upsert_external_card` loop →
      `{provider, project, created, updated}`). Reused `forge::{ForgeKind, classify_remote,
      forge_get, token_for}` — no new HTTP. Emits `board.binding.created` /
      `board.binding.deleted` / `board.sync.completed`. All network I/O precedes any store
      write (fails-loud ⇒ zero mutation).
- [x] **7. Wire.** `pub mod board_sync;` in `routes/mod.rs`; `.merge(routes::board_sync::router())`
      in `lib.rs::router()` (ADD only — `board::router()` left intact).
- [x] **8. Regression test (AC).** `tests/board_server_sync_016a.rs::post_board_sync_items_still_works_after_016a_merge`
      drives the **full merged router** and asserts `POST /api/board/sync {items:[…]}` still
      returns 200 + idempotent (no #58 regression). **PASS.**
- [x] **9. Fails-loud test (AC).** `tests/board_server_sync_016a.rs::server_sync_with_no_token_fails_loud_and_writes_nothing`
      seeds a card, binds a github repo, and triggers a pull with an empty `AGENTUM_HOME`
      (no token) → non-success status AND board card count + contents unchanged. **PASS.**
      (Also a handler-level twin + an unknown-binding-404 test in `board_sync.rs`.)
- [x] **10. Verify.** `cargo build -p agentum-core -p agentum-store -p agentum-server` clean.
      `cargo test … --lib`: core 40 / store 44 / server 326, **0 failed**. Integration file:
      2 passed, 0 failed. `cargo clippy -p agentum-server --lib` clean for new code (3 warnings
      are pre-existing in `board_goals.rs:336-338`, unrelated). Staged only own hunks.

## Deviations from architecture.md (with rationale)

- **`forge.rs` visibility widened to `pub(crate)`** (1 enum, 1 struct + 2 fields, 3 fns:
  `ForgeKind`, `Remote{api_base,project}`, `classify_remote`, `forge_get`, `token_for`).
  The architecture lists `forge.rs` under "Reuse (no change)", but on develop these items
  are **private**, so they can't be called from `board_sync.rs` without widening visibility.
  This is the smallest change that enables the mandated reuse (no behavior change), and it is
  exactly what the donor `feat/014d` branch did. No HTTP was reimplemented.
- **One compile-forced test-helper edit in `board_goals.rs`** (`card_with`): added the two
  new `BoardItem` fields as `None`. Mechanical (the new struct fields force it); `board_goals.rs`
  is not on the forbidden list. Forbidden files (`routes/board.rs`, `linear.rs`,
  `agentum-desktop/ui/**`) were **not** touched.
- **Shared crate-wide `crate::TEST_ENV_LOCK`** used by the fails-loud unit test (instead of a
  private mutex). The crate already serialises all `AGENTUM_HOME`-mutating tests through this
  lock; a per-module lock raced the planner-config read in `board_goals::tests` and caused a
  flaky failure. Using the existing shared lock fixed it.

## Out of scope (per non-goals — NOT done, by design)

- No push-back (`push_card` / `set_card_external_ref` / `status_to_state`) → 016b.
- No Linear (`linear.rs` untouched) → 016c.
- No desktop surface (no `ui/**` edits) → 016d.
- No GitLab, no pagination (>100 issues), no background/periodic sync, no webhooks,
  no `conflicts[]` field, no label/assignee/milestone mapping.
