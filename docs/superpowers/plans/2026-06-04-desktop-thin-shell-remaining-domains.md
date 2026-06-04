# Desktop thin-shell — remaining domains (post-git)

> Status 2026-06-04. The git de-dup is **done + committed** (`4f995e6` on
> `refactor/desktop-git-to-server`): the desktop's 1038-line git2
> reimplementation, `tauri/git.ts`, the contract `git:` namespace, the 33
> invoke_handler registrations, and the orphaned `git2` dep were deleted; git now
> runs on the embedded server (local → `server-git-adapter`, remote → RPC).
>
> This doc is the grounded plan for the *rest*, written after reading the actual
> code. It exists because the next steps need a **live desktop smoke test** that a
> headless agent can't run — each is a feature-build on a core surface, not a
> mechanical sweep.

## The key finding (read this first)

There is **no remaining clean "route to the cargos" de-dup**. Verified:

- **No** desktop command uses `reqwest`; **none** uses a shared `agentum_*` crate.
  Only `settings.rs` uses `rusqlite` (a local key-value `GlobalSettings` store the
  server's 2-field `preferences` route deliberately does **not** mirror). The one
  real crate-duplication was git2 — now removed.
- `gh`/`gl`/`usage`/`claude|codex|open_code_usage`/`linear`/`hooks`/`hosted_review`
  are **arg-less stubs** returning placeholder `Value`s ("not ported"). They are
  **not** reimplementations; the real logic exists server-side only for **remote
  runtime environments** (reached via `runtime-*-client.ts` RPC). For LOCAL the UI
  shows placeholders today.
- `repos.rs` / `worktrees.rs` are real, but they are **local JSON registries**
  (`~/.agentum/repos.json`, `~/.agentum/worktrees.json`) + git-CLI shelling + a
  **native folder dialog** (`tauri_plugin_dialog`, must stay native). The server
  has **no** `repos`/`worktrees` route — so these are net-new server features, not
  "route + delete".

Everything else (`pty`, `window`, `fs`, `shell`, `clipboard`, `keybindings`,
`browser`, `speech`, `notifications`, `app`, `updater`, dialogs) is legitimately
native OS glue and **stays**.

## Safe execution recipe (the proven git pattern)

For each domain, in order, never skipping the smoke test before deletion:

1. **Build the server route** in `crates/agentum-server/src/routes/<dom>.rs`,
   reusing crate plumbing (`run_git`/`cwd_for` style, `agentum_store`). Faithfully
   port the native logic so results match. Register in `lib.rs::router()`.
2. **Unit-test the pure logic** in Rust (parsing, selection, registry CRUD).
   Note: the server lib-test target is currently red from the concurrent
   `hook_base` work — verify pure fns standalone (`rustc --test`) until that lands.
3. **Add `ui/src/runtime/server-<dom>-client.ts`** (typed `getJson/postJson`)
   + route `runtime-<dom>-client.ts` **local → server with native fallback**
   (mirror `runtime-git-client` Slice A: `serverRead(server, local)` for reads).
4. **Verify**: `cargo check -p agentum-server -p agentum-desktop`, `vite build`,
   `tsc` (symlink `crates/agentum-desktop/shared→ui/src/shared` first — see git
   memory), `vitest src/runtime/`.
5. **Smoke-test the live desktop surface** (checklist per domain below).
6. **Only then** remove the native fallback + delete the native command + tauri
   namespace + contract entry + registration, and re-verify.

Priority: **repos → worktrees** (real logic, faithfully portable, high de-dup
value) then **forge** (server route exists, but UI flow is stubbed) then **usage**
(needs net-new scanning). Stubs (`linear`, `hooks`, `hosted_review`) only matter
once their server feature exists.

---

## 1. repos (`src/commands/repos.rs`, 349 lines)

**Current:** registry at `~/.agentum/repos.json` (`Repo{id,path,displayName,
badgeColor,addedAt,kind,connectionId,extra}`); CRUD = list/add/update/create/
clone/remove/reorder; git helpers = `repos_get_base_ref_default`,
`repos_search_base_refs`, `repos_search_base_ref_details` (shell git on repo.path);
native dialog = `repos_pick_folder`/`repos_pick_directory` (**stays native**);
stubs = `repos_clone_abort` (no-op), `repos_add_remote` (SSH error variant).

`runtime-repo-client.ts` already wraps only the **3 git-ref helpers** (local →
`api.repos.*`, remote → RPC `repo.baseRefDefault`/`repo.searchRefs`). CRUD is
called via `api.repos.*` directly, scattered in the UI.

**Plan:**
- New `routes/repos.rs`: read `~/.agentum/repos.json` (server shares the host
  file), port the registry CRUD + the 3 git-ref helpers. Endpoints e.g.
  `GET/POST/PATCH/DELETE /api/repos`, `POST /api/repos/reorder`,
  `GET /api/repos/{id}/base-ref-default`, `GET /api/repos/{id}/base-refs?q=&limit=`.
- Keep `repos_pick_folder` native (dialog) — the UI calls it separately.
- Server-client + extend `runtime-repo-client.ts` for the git helpers first
  (smallest, read-only, lowest risk), then a `runtime` wrapper for CRUD.
- **Delete only after smoke test**: list/add/remove/reorder/create + base-ref
  picker in New-Worktree/New-Session dialogs.

**Risk:** HIGH — this is the live **project list**. Registry format must round-trip
byte-identically (`extra` flatten, pretty + trailing `\n`). Keep native fallback
until verified.

**Smoke test:** open app → project list renders; add a folder; rename/recolor;
reorder; create a new repo (git + folder); remove; base-ref autocomplete in the
new-worktree dialog.

## 2. worktrees (`src/commands/worktrees.rs`, 500 lines)

**Current:** registry at `~/.agentum/worktrees.json` (+ resolves repoId via
`~/.agentum/repos.json`, + `worktree-sort-order.json`); git-worktree CLI ops —
`worktrees_list`/`list_all`/`update_meta`/`create`/`remove`/`list_lineage`/
`force_delete_preserved_branch`/`list_detected`/`resolve_pr_base`/`update_lineage`/
`persist_sort_order`. Several are **destructive** (`remove`, `force_delete_*`).

**Plan:** same as repos — port to `routes/worktrees.rs` over the shared registry
files + git CLI. Note the server `sessions` route already creates worktrees per
session (`createSession{worktree:true}`); reconcile so a worktree isn't tracked in
two places. **Depends on repos** (shares repoId resolution).

**Risk:** VERY HIGH — destructive git-worktree ops on the user's real trees. Port
faithfully, unit-test the registry + lineage logic, keep native fallback, and
smoke-test create/remove on a throwaway repo before deleting native.

**Smoke test:** create a worktree from a base ref; it appears in the sidebar; pin/
unpin (update_meta); remove it (and confirm the branch handling); detected-worktree
list; sort order persists across reload.

## 3. forge — gh + gl (`src/commands/gh.rs` 266, `gl.rs` 91)

**Current:** ~30 **arg-less stubs** returning placeholder `Value`s
(`gh_merge_pr`, `gh_update_issue`, `gh_pr_checks`, work-items, comments, reviews,
project views, …); only `gh_repo_slug` does real work. No `runtime-forge-client`.

**Server has:** `forge.rs` — `GET /api/forge/token`, and **session-scoped**
`/api/sessions/{id}/forge/{info,prs,pr,issues,checks}`.

**Plan:** the server covers ~5 of ~30 surfaces and is **session-scoped**, while the
UI's forge flow is itself stubbed (no real args wired). So this is **not** a
repoint — it's: (a) build a `runtime-forge-client.ts` like git; (b) map the UI's
forge actions to `ensureWorkspaceSession` + the forge endpoints; (c) build the
~25 missing forge endpoints (merge/auto-merge/reviewers/labels/project-items/…)
before native can be deleted. Token flow via `/api/forge/token`.

**Risk:** MED-HIGH (OAuth/token handling; PR mutations). Do read-only first
(info/prs/checks) behind fallback, verify, then mutations.

## 4. usage — claude/codex/open_code (`src/commands/*_usage.rs`, ~213)

**Current:** **stubs** — `claude_usage_get_{scan_state,summary,daily,breakdown,
recent_sessions}` etc. return zeroed/empty shapes so the panes render "no data".

**Server has:** `usage.rs` — `/api/usage` (UsageBundle, chip-level), `/api/usage/
claude` (plan-limit %), `/api/usage/codex`. This is the sidebar chip, **not** the
detailed per-day/per-model/per-project analytics the panes want.

**Plan:** net-new — extend `crate::usage` to aggregate `~/.claude/projects` &
`~/.codex/sessions` JSONL into summary/daily/breakdown/recent-sessions (cost needs
a model-price table). Add endpoints, wire the panes. **Unit-test the aggregation in
Rust** (this part *is* headless-verifiable). Then delete the stubs.

**Risk:** MED — cost/model accounting is domain logic; validate numbers against a
known session before trusting the panes.

---

## Out of scope (stays native, do not touch)
`pty` `window` `fs` `shell` `clipboard` `keybindings` `browser` `speech`
`notifications` `app` `updater` `diagnostics` `settings` (local KV) `accounts`
(live) `telemetry` (live) + all dialogs.
