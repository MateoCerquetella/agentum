# Tasks — Spec 014 (per-project persistent browser profiles)

Build order = `architecture.md` §4. One harness feature per /sdd-loop
developer iteration; each step independently green.

## Feature 1 — `per-project-cdp-profile` (AC 1, 2, 6) — ✅ CODE-COMPLETE 2026-07-09

- [x] **Step 1 — scope layer**: `BrowserScope{Shared,Project,Adhoc}` +
  `profile_token()` (prefix AFTER sanitize) + pure `resolve_scope_from_tables`
  + `resolve_path_via_git` + `resolve_scope_with` + `resolve_browser_scope`
  (`cdp_browser.rs`); table accessors `scope_worktree_pairs`
  (`routes/worktrees.rs`) / `scope_repo_pairs` (`routes/repos.rs`). 7 scope
  tests incl. the pane-id/agent-path unification contract + git-fallback
  (temp `git init` + `worktree add`, canonicalized both sides).
- [x] **Step 2 — shared profile relocation**: `user_data_dir()` →
  `state_dir()/cdp-browser/shared` (Decision G; `stop_local_cdp_browser`
  needed zero edits) + `shared_user_data_dir_is_nested_shared_subdir` pin.
- [x] **Step 3 — launch re-key**: `ensure_local_cdp_browser_for` resolves the
  scope; registry renamed `browser_registry` keyed by TOKEN (struct →
  `ScopedBrowser`, `register_scoped_browser`, `profile_dir_for_token`);
  opt-out env checked BEFORE resolution (same semantics, no registry reads
  when off — documented deviation from the architecture's "after", effect
  identical per its own note).
- [x] **Step 4 — teardown split (AC 2, riskiest)**: scope-aware
  `stop_local_cdp_browser_for` (Project: attach-count-gated stop, NEVER
  deletes the dir; Adhoc: kill+delete verbatim; Shared: no-op) +
  `attach_counts` + `BrowserAttachGuard` (Drop = decrement) +
  `register_browser_attach`; guard created in `routes/cdp_screencast.rs::
  screencast` and MOVED INTO `run()`'s future; remove caller passes
  `body.worktree_id` (full id), prune passes `{repo_id}::{path}`. Tests:
  never-deletes / adhoc-deletes / release-noop-while-attached.
- [x] **Step 5 — boot sweep**: `sweep_legacy_profile_dirs` (top-level
  non-`shared`/non-`project-*` entries only, dirs AND files) wired strictly
  AFTER `reap_orphaned_cdp_browsers` in `agentum-desktop/src/lib.rs` boot
  task + `sweep_deletes_only_legacy_entries` (incl. idempotency).

**Gates (all green 2026-07-09):**
- `cargo test -p agentum-server --lib` → **572 passed / 0 failed / 5 ignored**
  (11 new spec-014 tests; pre-existing suites incl.
  `canonical_worktree_key_unifies_pane_id_and_agent_path` + `chrome_argv_…`
  held).
- `cargo check -p agentum-desktop` → clean (after the known worktree
  sherpa/onnxruntime dylib copy from the main checkout's `target/release/`).
- `cargo fmt --all --check` → clean; `cargo clippy -p agentum-server --lib`
  → no warnings.

**Deviations (documented):**
1. Opt-out env check placed BEFORE scope resolution (architecture said
   "moves after resolution but keeps the same effect") — chosen so the
   opt-out path never reads the registries; semantics identical.
2. Desktop gate ran as `cargo check` (full type-check of the crate), not
   `cargo build` — the only desktop change is two boot-task lines; link step
   deferred to the release build.
3. Env-mutating tests read the REAL `~/.agentum/*.json` registries inside
   `resolve_browser_scope` (they live under `HOME`, not `AGENTUM_HOME`) —
   read-only and outcome-deterministic for the raws used (`::`-prefixed and
   pseudo-key inputs never consult the tables); the injectable
   `resolve_scope_with` core carries the table-sensitive assertions.

## Feature 2 — `plain-workspace-and-native-routing` (AC 3, 4) — ✅ CODE-COMPLETE 2026-07-09

- [x] **Step 6 — UI project derivation + surface verification**: traced EVERY
  `createBrowserTab` call site — all pass store worktree ids, which are
  `<repoId>::<path>` for anything project-backed (plain workspaces included,
  per the worktrees registry id format); the only non-project ids are the
  synthetic `global-floating-terminal` / `__orphan__` (correctly Adhoc =
  pre-014 isolation) → **no surface fix needed**. New pure
  `ui/src/lib/browser-project.ts::deriveProjectRepoId` (built on the existing
  `shared/worktree-id::splitWorktreeId`) + `browser-project.test.ts` (5 tests
  — the AC 7 derivation test; F3's clear action will consume it).
- [x] **Step 7 — native re-key**: `project_store_token` (`::` prefix →
  `project-<repoId>`; bare UUID → `project-<uuid>`; else raw per-key fallback)
  now feeds `worktree_data_store_id`'s SHA-256; call-site comments updated.
  3 desktop unit tests incl. the legacy-id disjointness pin.

**Gates (all green 2026-07-09):**
- `bunx vitest run src/lib/browser-project.test.ts` → 5/5; browser-pane suite
  86/88 — the 2 fails are `webview-registry.test.ts` PRE-EXISTING baseline
  (zero existing UI files touched; git status shows only the 2 new files).
- `bun run build` (vite) → ✓ 1m09s (deps via `bun install`, node_modules was
  absent in this worktree).
- `cargo test -p agentum-desktop --lib` → 78/0 (3 new `project_store_*`).
- `cargo fmt --all` applied + `--check` clean.

**Deviation:** none vs architecture — step 6's "verify/fix" resolved to
verify-only (the surfaces already pass project-scoped ids).

## Feature 3 — `clear-browser-data-action` (AC 5) — ✅ CODE-COMPLETE 2026-07-09

- [x] **Step 8 — server**: `cdp_browser::clear_project_browser_data` (bails on
  empty id; force-stop IGNORING attach counts — explicit user intent; then
  `remove_dir_all` of ONLY that project's dir, errors propagate) +
  `POST /api/cdp-browser/clear-project-data` (`routes/cdp_browser.rs`,
  body `{repoId}`, authed like the rest). Tests: only-mine (project-q intact,
  even with a LIVE attach on project-p) + empty-id bail.
- [x] **Step 9 — native**: `store_tokens()` label→token registry (populated in
  `browser_webview_open`, pruned in `browser_webview_close`, liveness-filtered
  at read) + `browser_clear_project_data` command (FLAT named params) using
  `Webview::clear_all_browsing_data()` (confirmed present in pinned tauri
  2.11.2) on a live webview of the project's store; no live webview →
  `{cleared:false, warning}` — observable degradation per AC 5. Registered in
  `lib.rs` `generate_handler!`. Stub `browser_session_delete_profile` flipped
  `true`→`false` (honest failure; legacy profiles UI now shows real failure).
- [x] **Step 10 — UI**: `clearProjectCdpData` in
  `runtime/cdp-screencast-client.ts` (NOT fire-and-forget — errors returned);
  screencast toolbar gets a Trash2 icon button (rendered only when
  `deriveProjectRepoId(worktreeId)` ≠ null) + shadcn confirm `Dialog`
  (destructive action wording: "every workspace of this project is signed
  out"); handler runs server clear then native clear via
  `api.browser.clearProjectData` (auto-derived command) and toasts the
  aggregate — any native warning shown VERBATIM.

**Gates (all green 2026-07-09):**
- `cargo test -p agentum-server --lib` → **574/0/5** (2 new clear tests).
- `cargo test -p agentum-desktop --lib` → 78/0; clippy desktop lib clean.
- vitest browser-project + browser-pane → 86/88, the same 2 PRE-EXISTING
  `webview-registry.test.ts` baseline failures as before F3 (delta 0).
- `bun run build` (vite) → ✓ 1m09s (typechecks the pane edits).
- `cargo fmt --all --check` → clean.

**Deviations (documented):**
1. UI affordance = a direct trash-icon button + confirm Dialog instead of the
   architecture's one-item `…` DropdownMenu — same single surface + confirm
   step, less chrome for a menu that would hold exactly one item.
2. No new UI unit test for the toolbar handler (imperative glue over the
   already-tested `deriveProjectRepoId` + runtime fn; vite build covers the
   typing) — the end-to-end behavior is qa.sh territory per the spec's
   harness section.

## Developer phase — ✅ COMPLETE (F1+F2+F3), 2026-07-09 → tester
