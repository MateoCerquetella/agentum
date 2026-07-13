# Handoff 03 — Developer → Tester

- **Spec:** 014-per-project-browser-profiles
- **Date:** 2026-07-09
- **From:** Developer (autonomous /sdd-loop iterations 3–5, implemented INLINE)
- **To:** Tester
- **Artifacts:** commits `8dbe0d88` (F1), `a69538b5` (F2), + the F3 commit on
  branch `hagfish`; `tasks.md` (per-step status, gates, 6 documented
  deviations); code per `architecture.md` §4 build order (all 10 steps).

## What was built (by feature)

- **F1 `per-project-cdp-profile` (AC 1/2/6)** — `BrowserScope` + resolution
  chain (`cdp_browser.rs`), token-keyed `browser_registry`
  (`project-<repoId>`), shared profile → `cdp-browser/shared/`, teardown split
  (attach-refcount via `BrowserAttachGuard` riding in the screencast `run()`
  future; project dirs NEVER deleted on stop; Adhoc keeps kill+delete),
  remove/prune callers pass full ids, boot sweep after reap
  (`agentum-desktop/src/lib.rs`).
- **F2 `plain-workspace-and-native-routing` (AC 3/4)** — surface audit
  (verify-only: all project-backed surfaces already send `<repoId>::<path>`),
  pure `ui/lib/browser-project.ts::deriveProjectRepoId`,
  `browser_native.rs::project_store_token` feeding `worktree_data_store_id`.
- **F3 `clear-browser-data-action` (AC 5)** — server
  `clear_project_browser_data` + `POST /api/cdp-browser/clear-project-data`;
  native `store_tokens` registry + `browser_clear_project_data` (flat args,
  `clear_all_browsing_data()` through a live webview, observable warning
  otherwise); stub `browser_session_delete_profile` flipped to honest `false`;
  screencast-toolbar trash button + confirm dialog + verbatim-warning toasts.

## Final gate numbers (developer-run, tester should re-run)

- `cargo test -p agentum-server --lib` → 574 passed / 0 failed / 5 ignored
  (13 new spec-014 tests).
- `cargo test -p agentum-desktop --lib` → 78/0 (3 new).
- vitest: `browser-project.test.ts` 5/5; browser-pane suite 86/88 — the 2
  fails are `webview-registry.test.ts` PRE-EXISTING baseline (unchanged
  before/after every feature; no existing UI file behind them was touched).
- `bun run build` (vite) ✓; `cargo fmt --all --check` ✓; clippy (server +
  desktop libs) clean.
- Worktree gotchas the tester will hit: desktop cargo needs the sherpa +
  onnxruntime dylibs copied into `target/release/` (done in this worktree);
  UI deps via `bun install`; use `$HOME/.cargo/bin/cargo`.

## Deviations from architecture (all documented in tasks.md)

1. Opt-out env check before (not after) scope resolution — same semantics.
2. Desktop gate = `cargo check`/`--lib` tests, not a full binary build.
3. Two env-isolated tests read the real `~/.agentum/*.json` (read-only,
   outcome-deterministic; the injectable core carries table assertions).
4. F2 step 6 resolved verify-only (no surface fix was needed).
5. F3 UI = direct trash button + confirm Dialog, not a one-item DropdownMenu.
6. No unit test for the F3 toolbar handler (glue over tested parts; qa.sh
   covers the flow).

## Tester focus (per AC, suggested verification)

1. **AC 1** — unit evidence: `project_profile_token_…`, `scope_…` tests; spot
   the launch path (`ensure_local_cdp_browser_for`) builds
   `cdp-browser/project-<repoId>`.
2. **AC 2** — `stop_project_scope_never_deletes_profile_dir`,
   `release_is_noop_while_project_attached`; verify the guard is MOVED into
   `run()`'s future (`routes/cdp_screencast.rs`) — not dropped at handler
   scope; verify remove (`routes/worktrees.rs:~444`) passes `body.worktree_id`
   and prune passes `{repo_id}::{path}`.
3. **AC 3** — `scope_miss_is_adhoc_never_shared` (miss ≠ shared) + the F2
   surface audit trace in tasks.md.
4. **AC 4** — `project_store_ids_stable_and_distinct_per_repo`,
   `project_store_id_never_equals_legacy_worktree_id`.
5. **AC 5** — `clear_project_browser_data_deletes_only_that_project` (incl.
   live-attach), empty-id bail; native degradation path returns an observable
   warning; stub now returns `false`.
6. **AC 6** — `sweep_deletes_only_legacy_entries` (idempotent); reap unchanged
   (process-killer only); hermetic CDP self-test untouched
   (`cdp_driver.rs:1286-1310`).
7. **AC 7** — re-run all gates independently; confirm the 2 vitest fails are
   the pre-existing baseline (git blame shows no touch).

**Live/browser checks (qa.sh / human, NOT this tester phase):** actual
login-persistence across tab close + relaunch, cross-project isolation in a
real session, and the clear action end-to-end in the running app — per the
spec's Harness wiring section.

## Sacred surfaces to confirm untouched

`pkill_by_signature` containment; `canonical_worktree_key` + its `:804` test;
`reap_orphaned_cdp_browsers` behavior; push-based streaming (no polling
added); remote SSH browser path; `spawn_agent_into_pane`/YOLO/MCP wiring;
screencast remains the default browser surface.
