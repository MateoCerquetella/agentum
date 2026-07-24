# Handoff: Developer → Tester — Spec 016a

## 1. Summary
016a **implemented, GREEN, committed** (worktree `.claude/worktrees/016a-board-server-pull`,
branch `feat/016a-board-server-pull` off `origin/develop` @ 0ad4d58, commit **`059bf00`**,
9 files, +1214/−10, **NOT pushed**). Server-side GitHub pull + durable bindings + migration
`0023`, built on top of #58. Ready for independent acceptance testing.

## 2. Completed Work
- Migration `0023_board_external_two_way.sql` — `external_id` + `external_synced_at` columns,
  `board_tracker_bindings` table, `(external_provider, external_id)` reconcile index.
- core: `BoardItem` +2 fields, `TrackerBinding`. store: 5 helpers + 2 tests.
- NEW `routes/board_sync.rs`: pure reconcile core (ported with unit tests) + github-only
  bindings CRUD + `sync_binding` on `POST /api/board/bindings/{id}/sync`; reuses
  `forge::{classify_remote, forge_get, token_for}`; all network I/O precedes any store write.
- Router wired (ADD only); #58's `POST /api/board/sync {items}` untouched.

## 3. Pending Work (Tester)
Verify the **016a-scoped** acceptance criteria only:
- Bind a board to a GitHub repo; binding persists (store roundtrip).
- Server-side pull imports issues as cards; re-sync is idempotent (no duplicates).
- Tracker unreachable / no token → fails loud, **zero board mutation**.
- #58 `POST /api/board/sync {items}` still works (no regression).
- **Out of 016a scope (do NOT fail the gate on these):** push-back (016b), conflict
  `conflicts[]` policy (016b), Linear (016c), desktop surface (016d).

## 4. How to test (entry points)
- In the worktree: `cargo test -p agentum-core -p agentum-store -p agentum-server --lib`
  and `cargo test -p agentum-server --test board_server_sync_016a`.
- Key tests: `tests/board_server_sync_016a.rs` (the 2 AC integration tests:
  `post_board_sync_items_still_works_after_016a_merge`,
  `server_sync_with_no_token_fails_loud_and_writes_nothing`); `board_sync.rs` unit tests
  (reconcile/parse/binding accept-reject/404); store tests
  (`upsert_external_card_is_idempotent_on_re_sync`, `tracker_binding_roundtrip_and_rebind`).
- Live `gh` paths are `#[ignore]`/token-gated — suite passes offline.
- Reported by developer: core 40 / store 44 / server 326 (+5 ignored) + integration 2, **0 failed**.

## 5. Risks / scrutinize
- 3 documented deviations (`forge.rs` `pub(crate)` visibility — no behavior change;
  `board_goals.rs` compile-forced test-helper; shared `TEST_ENV_LOCK`). Confirm none change behavior.
- **Binding "persists across daemon restart"** AC: covered at the store level
  (`tracker_binding_roundtrip_and_rebind`) rather than a true process restart — judge whether
  the store-roundtrip is sufficient evidence or flag it.

## 6. Questions
- None blocking.

## 7. Recommended Next Step
Tester runs the suite in the worktree, returns a pass/fail verdict per 016a-scoped AC with
repro steps, then hands to **Reviewer**. Push/promote stays human-gated.
