# Handoff 01 — PM → Architect

- **Spec:** 014-per-project-browser-profiles
- **Date:** 2026-07-09
- **From:** PM (autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/014-per-project-browser-profiles/spec.md` (PM-gated; decisions D1–D4 locked)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all nine items green
after edits. Every code citation independently re-verified against the tree;
three fixed in the spec (`stop_local_cdp_browser_for` :450 not :448,
`build_chrome_argv` :580 not :410, `sanitize_worktree_token` :280 /
`pkill_by_signature` :416) and one wrong path claim corrected (local shared
profile = `state_dir()/cdp-browser`, `cdp_browser.rs:774–781` — on macOS
`state_dir()` falls back to the data dir, `agentum-store/src/paths.rs:56–64`;
`$HOME/.agentum/cdp-browser` is only the REMOTE host path, `:493`).

One-slice note: the clear action (feature 3) is severable but must NOT be
split off — shipping persistence without a deleter creates an un-clearable
cookie store (privacy regression).

## Decisions locked (see spec "Decisions (PM-locked)")

D1 key by **project** (worktrees of one repo share logins; deliberate
inversion of v0.27-era per-worktree isolation — changelog it). D2 project
identity = registry `Repo.id` UUID (`routes/repos.rs:140`, idempotent-by-path
`:126–128`, carried as `repo_id` on every worktree row `routes/worktrees.rs:50`
and in the UI worktree id `<repoId>::<path>` `:371`); repo remove+re-add mints
a new UUID → orphaned profile = "new project", accepted. D3 retention: the
explicit clear action is the ONLY deleter — orphan reap stays a
process-killer, repo removal does not cascade-delete. D4 migration: one-time
boot sweep deletes legacy per-worktree dirs; new dirs use the `project-`
prefix so the sweep is unambiguous and never touches the shared root dir or
temp test profiles.

## Material PM findings (architect focus)

1. **Teardown scoping is the second hard problem** (beyond the Chromium
   single-instance lock): `stop_local_cdp_browser_for` fires per-worktree from
   three call sites — last-tab close (`routes/cdp_browser.rs:92`, client
   `ui/src/runtime/cdp-screencast-client.ts:287–298`), worktree remove
   (`routes/worktrees.rs:444`), prune (`routes/worktrees.rs:624`). One process
   per project + per-worktree triggers = worktree A's close kills the browser
   worktree B's pane is screencasting. Needs a project-scoped attach/refcount
   before stopping the process. (AC 2 rewritten to pin this.)
2. **Single-instance lock shape confirmed**: `worktree_registry()`
   (`cdp_browser.rs:255–258`) maps key→(port, tmux, profile);
   `ensure_local_cdp_browser_for` (`:331–394`) is idempotent per key with port
   reuse. Re-keying key=project yields exactly one process/port/profile per
   project — no two processes ever share a `--user-data-dir`.
3. **Agent side carries no repo id**: `canonical_worktree_key`
   (`cdp_browser.rs:271–277`) exists because MCP agents send a bare
   `worktree_path` while panes send `<repoId>::<path>`. New keying inverts
   this: keep the repoId, resolve bare paths → `repo_id` (registry row whose
   id ends `::<path>`; `git rev-parse --git-common-dir` + repos.json path
   match fallback). `github-pr:repo:42` pseudo-keys (test `:824`) have no
   path — preserve per-key behavior for them.
4. **Shared root profile**: `user_data_dir()` = `state_dir()/cdp-browser`
   (`cdp_browser.rs:774–781`). Note `stop_local_cdp_browser` (`:209–223`)
   also deletes the shared dir on stop — decide whether that stays for the
   truly contextless browser.
5. **AC 5 native half feasibility**: `data_store_identifier` set at webview
   build (`browser_native.rs:168`); clearing needs a WKWebsiteDataStore
   remove-by-identifier API through Tauri — verify availability; degradation
   path (observable warning, never silent success) is defined in AC 5. Stub
   to replace: `browser_session_delete_profile`
   (`crates/agentum-desktop/src/commands/browser.rs:37`).
6. **Env opt-out**: `AGENTUM_BROWSER_PER_WORKTREE` (`cdp_browser.rs:340–342`)
   needs a defined post-change meaning (suggested: unchanged — everything
   shares the one root browser).
7. **Identity stability boundary**: `Repo.id` survives relaunch + worktree
   churn but not repo remove+re-add (`repos.rs:126–140`) — accepted in D2; do
   NOT "fix" by switching to path/URL keying.

## Architect deliverable

`ai/specs/014-per-project-browser-profiles/architecture.md`: boundaries +
tradeoffs for (a) the project-keyed browser registry (one process per
project), (b) the project-scoped teardown mechanism (refcount vs
attach-registry) replacing the three per-worktree stop triggers, (c) the
bare-path→project resolution chain, (d) the clear-browser-data command/route
end-to-end (server + native + UI), (e) the D4 boot sweep. Respect the pkill
safety invariant (profile paths stay under `cdp-browser/`), the hermetic CDP
self-test (`cdp_driver.rs:1302`), and `AGENTUM_HOME` test isolation.
