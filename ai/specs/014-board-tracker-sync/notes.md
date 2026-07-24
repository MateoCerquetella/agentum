# 014 notes / progress ledger

Read this first each Ralph iteration — it's the source of truth for what's done
and what's next, so the loop advances instead of redoing work.

## Status (2026-06-22)

- **Parent 014 spec**: written (`spec.md`). PM gate: content PASS, "fits one
  screen" fails by design → SPLIT 014a–e.
- **014a architecture**: written (`architecture.md`).
- **014a implementation**: ✅ DONE + GREEN + **PR #68 → develop** (commit
  0d16477 on `feat/014a-board-tracker-sync`). Pull/import.
- **014b implementation**: ✅ DONE + GREEN + **PR #69 (stacked on #68, base
  `feat/014a`)** (commit `eda31ea` on `feat/014b-board-tracker-push`).
  GitHub push-back → GitHub two-way loop complete. Clean-worktree verify:
  store 43 / server 250 pass; clippy clean.
- **014c implementation**: ✅ DONE + GREEN + **PR #71 (stacked on #69)** (commit
  `b888851` on `feat/014c-board-tracker-linear`). Linear two-way (pull + push),
  ported from staging's Linear client + decoupled from the harness phase enum.
  Clean-worktree verify: store 43 / server 257 pass; clippy clean.
- **NEXT: 014d (desktop board UI)** — needs a desktop board view that reads
  `/api/board` (none today) + bind/sync/external-link UI; requires the bun build
  + human visual QA. 014e (auto-sync, conflict UI, GitLab, field maps) deferred.

### Pre-existing breakage (NOT 014a, do not "fix")
`cargo clippy --all-targets` fails to compile two integration tests
(`tests/goal_cards_end_to_end.rs`, `tests/card_session_binding_e2e.rs`) with
`E0063: missing field mcp_token in initializer of AppState`. That field is in
committed HEAD's `AppState`; those tests are stale and unrelated to 014a (they
build no `BoardItem`). Use `--lib` to verify 014a cleanly.

## Build order
014a (foundation + GitHub pull) → 014b (GitHub push-back + conflict policy) →
014c (Linear) → 014d (desktop board surface) → 014e (deferred).

## 014a checklist — ✅ ALL DONE
- [x] migration `0022_board_external_sync.sql` (external_* cols + bindings table)
- [x] core: `BoardItem.external_*` + `TrackerBinding`
- [x] store: `BoardItemRow`/`try_from`; `upsert_external_card`,
      `list_external_refs`, binding CRUD + tests
- [x] forge.rs: expose `pub(crate)` helpers (`ForgeKind`, `Remote`,
      `classify_remote`, `forge_get`, `token_for`)
- [x] `routes/board_sync.rs`: reconcile (pure) + bindings/sync routes + tests
- [x] wire `routes/mod.rs` + `lib.rs`
- [x] fix `board_goals.rs` test helper (`card_with`) for new BoardItem fields
- [x] `cargo test -p agentum-store -p agentum-server --lib` green; clippy clean

## 014a files (touched — for review / staging; stage ONLY these)
- `crates/agentum-store/migrations/0022_board_external_sync.sql` (new)
- `crates/agentum-core/src/lib.rs` (BoardItem +4 fields, +TrackerBinding)
- `crates/agentum-store/src/lib.rs` (row+try_from+create Ok-block; new methods+tests)
- `crates/agentum-server/src/routes/board_sync.rs` (new)
- `crates/agentum-server/src/routes/forge.rs` (pub(crate) exposure only)
- `crates/agentum-server/src/routes/mod.rs` (+`pub mod board_sync;`)
- `crates/agentum-server/src/lib.rs` (+1 `.merge(...)` line)
- `crates/agentum-server/src/routes/board_goals.rs` (test helper +4 fields)
- `ai/specs/014-board-tracker-sync/{spec,architecture,notes}.md`

## 014b — DONE (GitHub push-back)
Shipped: `forge_send` (forge.rs), `set_card_external_ref` (store),
`status_to_state`/`parse_repo_from_issue_url`/`push_card`/`resolve_push_target`
(board_sync) + `POST /api/board/{id}/push`. Files touched: forge.rs,
board_sync.rs, store/lib.rs (all staged hunk-disciplined; foreign WIP untouched).

## 014c — DONE (Linear two-way)
Ported staging's Linear client (was `agentum-server/src/linear.rs` +
`agentum-desktop/src/commands/linear.rs`) into NEW `agentum-server/src/linear.rs`,
**decoupled** from `task_sink::TrackerPhase` (board uses todo/doing/done, mapped
to Linear state **types**). Added a pull query (staging was push-only). No `dirs`
dep (data-local dir computed directly → no Cargo.toml/lock churn). board_sync's
reconcile is now provider-agnostic (column-based). Files: NEW linear.rs,
board_sync.rs (refactor), lib.rs (+`pub mod linear;`). PR #71.

## 014d (desktop board UI) — the last buildable slice
No desktop view reads `/api/board` today. Needs: a board view (cards by column),
a bind-tracker control (provider + project/team), a "Sync now" button
(`POST /api/board/sync`), a per-card external-link badge, and a per-card "Push"
(`POST /api/board/{id}/push`). Frontend-only; verify with `bun run build` +
human visual QA. Thin client mirrors `board-client.ts`/`harness-client.ts`.

## 014e — deferred (background auto-sync, conflict-resolution UI, GitLab,
label/assignee/comment field maps, webhooks, two-sided conflict detection).

## Hard constraints (don't regress)
- Working tree has **foreign WIP** (009c-3 screencast + many M files). Touch ONLY
  014a files. **Never `git add -A` / commit / stash.** Push is human-gated.
- Reuse `forge.rs` (REST + token + normalize) — do NOT shell `gh` or rebuild it.
- Don't modify `NewBoardItem` (≈40 call sites) — use `upsert_external_card`.
- 011's `TaskSink` push-seam is on **staging**, not this `main` checkout; 014a
  (pull) doesn't need it, but 014b (push-back) will → confirm branch first.

## Known scope limits for "100%"
Full 014 includes 014d (a desktop board view that does NOT exist today — nothing
reads `/api/board` in the desktop UI) and 014c (Linear, whose `linear.rs` is
staging-only). Live end-to-end needs real gh/Linear tokens + a running desktop.
Those crossing-branch / human-gated steps cap what is autonomously verifiable.
