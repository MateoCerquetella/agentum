# Spec 014 — Per-project persistent browser profiles

- **Number:** 014
- **Status:** Done  <!-- Reviewer SIGN-OFF 2026-07-09 (review.md) — SHIP-READY on base d957eefd; release = human (rebase/port onto fresh origin/develop + qa.sh) -->
- **Surface:** `crates/agentum-server` (cdp_browser.rs) + `crates/agentum-desktop` (src/commands/browser_native.rs, ui/src/components/browser-pane/)
- **Author:** Mateo (drafted via /sdd-spec)
- **Date:** 2026-07-09

## Problem

Browser identity is keyed to the **ephemeral worktree**, and the profile is
destroyed with it: `stop_local_cdp_browser_for` (`cdp_browser.rs:450`,
`remove_dir_all` at `:466`) deletes the per-worktree Chromium profile dir on
worktree removal **and** when the user closes the last browser tab — so a
staging login is wiped the moment the tab closes. Meanwhile anything browsed
without worktree context falls back to the ONE shared root profile
(`state_dir()/cdp-browser`, `cdp_browser.rs:774–781` — NOT `~/.agentum/...` on
macOS unless `AGENTUM_HOME` is set), leaking cookies and logins across every
project. There is no durable, project-scoped browser state and no user-facing
way to clear it per project.

> Note (corrects the raw ask): per-**worktree** isolation already exists on both
> surfaces — each worktree gets its own Chromium `--user-data-dir`
> (`cdp_browser.rs:306`) and its own WKWebView data store
> (`browser_native.rs:48`). The gap is **persistence** and **project-level
> keying**, not isolation itself.

## Goal

Browser profiles are keyed to the **project** and persist across browser-tab
close, worktree teardown, and app relaunch — with a project-scoped
"Clear browser data" action.

## Users / personas

An engineer driving agents across several projects at once, each targeting a
different staging environment/account. They log into project A's staging app in
a browser pane, close the tab (or the worktree finishes and is removed), reopen
a browser later in the same project — and expect to still be logged in, while
project B's browser stays logged into *its* account, untouched.

## Acceptance criteria

1. Opening a browser pane in any workspace of project P launches (or attaches
   to) a Chromium whose `--user-data-dir` is a **per-project** dir
   `agentum_store::paths::state_dir()/cdp-browser/project-<repoId>/`, keyed by
   the registry `Repo.id` UUID (PM decision D2); browser panes in workspaces
   of two *different* projects get two *different* dirs (no cookie/storage
   leakage between projects — existing isolation property preserved,
   re-keyed). The `project-` prefix makes new dirs distinguishable from legacy
   per-worktree dirs (D4).
2. None of `stop_local_cdp_browser_for`'s three callers (last-tab close
   `routes/cdp_browser.rs:92`; worktree remove `routes/worktrees.rs:444`;
   prune `routes/worktrees.rs:624`) deletes the project profile dir any
   longer. The project's Chromium process + tmux session are stopped only when
   **no workspace of that project** still has a browser pane attached —
   closing worktree A's last tab must not kill the browser worktree B's pane
   is live-screencasting. A login performed before close survives a tab close,
   worktree removal, and a full app relaunch.
3. Browsing outside any worktree (plain workspace / project hub) routes to
   **that project's** profile dir, never the shared root
   `state_dir()/cdp-browser` profile (which remains only for truly
   project-less contexts, e.g. a contextless MCP `agentum_browser` call from a
   non-worktree agent).
4. The native-webview surface derives its WKWebView `data_store_identifier`
   from the same project identity (same SHA-256-truncate scheme as today's
   `worktree_data_store_id`, `browser_native.rs:48`), so native tabs of one
   project share one store and persist across relaunch.
5. A "Clear browser data" action scoped to the project exists end-to-end: a
   real command/route (replacing the hardcoded stub
   `browser_session_delete_profile`, `commands/browser.rs:37`) that kills that
   project's browser process and deletes **only** that project's profile dir
   (and native data store), leaving every other project's profile intact;
   invoking it emits an observable success/failure result to the UI. If
   Tauri/WKWebView exposes no API to remove a data store by identifier, the
   native half may degrade to a documented no-op **with an observable "native
   store not cleared" warning in the result** — silent hardcoded success
   (today's stub behavior) is the failure mode being removed.
6. `reap_orphaned_cdp_browsers` (boot/quit sweep, wired at desktop
   `lib.rs:130`) still kills every orphaned agentum-launched Chromium, and the
   hermetic CDP self-test keeps using its own temp profile
   (`cdp_driver.rs:1302`) — neither regresses.
7. `npm run build --prefix crates/agentum-desktop/ui` completes with no
   TypeScript errors; existing browser-pane unit tests in
   `ui/src/components/browser-pane/` stay green; at least one **new** test
   covers the profile-keying/routing logic (pure derivation: workspace →
   project profile key); `cargo test -p agentum-server --lib` stays green with
   at least one new test on project-key derivation + the no-delete-on-stop
   behavior.

## Scope & non-goals (YAGNI)

- **In:** per-project CDP profile keying + persistence; plain-workspace
  routing; native data-store re-keying; project-scoped clear action; tests.
- **Out:**
  - User-agent overrides (the raw ask marked them "optionally" — cut).
  - Cookie import/export from the user's real browser (`browser.rs` import
    stubs stay stubs).
  - Multiple named profiles per project ("work"/"personal") — one profile per
    project.
  - Remote/SSH host browsers (spec 009-host-resident-browser territory).
  - TUI surface (separate repo).
  - Any change to the screencast-vs-native default (CDP screencast **is** the
    browser — hard lesson, do not regress).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- Per-worktree Chromium launch: own port + tmux + profile dir
  (`cdp_browser.rs:233–407`, `worktree_profile_dir` at `:306`,
  `build_chrome_argv` isolated `--user-data-dir` at `:580`). Re-key, don't
  re-implement.
- Token sanitization + pkill-by-profile-signature teardown
  (`sanitize_worktree_token` `:280`, `pkill_by_signature` `:416`, safety doc
  `:409–415`) — the safety
  property that we only ever kill processes whose cmdline references *our*
  `cdp-browser` state dir must carry over unchanged.
- Boot/quit orphan reap (`reap_orphaned_cdp_browsers` `:438`; caller
  `agentum-desktop/src/lib.rs:130`).
- Native per-worktree WKWebView data store: `worktree_data_store_id`
  (SHA-256 → 16 bytes, `browser_native.rs:42–50`) + `data_store_identifier`
  wiring (`:168`). Same scheme, new key input.
- UI panes already thread `worktreeId` through every browser-pane component
  (`BrowserPane.tsx`, `NativeBrowserPagePane.tsx`,
  `AgentBrowserScreencastPane.tsx`, …) — the id plumbing exists; it needs a
  workspace→project resolution step, not new plumbing.
- Hermetic CDP self-test with temp profile (`cdp_driver.rs:1284–1310`) — the
  pattern for profile-isolated tests.

### Build new

- A **project-identity key** helper: resolve a worktree/workspace to its
  project (canonical repo root or registry repo id) and derive the profile
  token from it (exact identity source = architect decision, see O1/O2).
- The **persistence policy change**: split "stop the browser process" from
  "delete the profile" in `stop_local_cdp_browser_for`.
- **Plain-workspace routing** to the project profile (today it falls through
  to the shared root profile).
- A **real clear-browser-data command/route + UI affordance** (project
  settings / browser toolbar menu), replacing the `browser.rs:37` stub.
- Tests: pure key-derivation (TS + Rust), stop-without-delete, clear-only-mine.

## Risks & invariants

- **Chromium single-instance lock:** two Chromium processes cannot share one
  `--user-data-dir`. Keying the profile by project means two concurrent
  worktrees of the same project must share **one browser process** (registry
  re-keyed project→browser), not two processes on one dir. This is the core
  architectural decision — flagged for the architect, not solvable by a path
  rename alone.
- **Teardown-trigger conflict (core, for the architect):**
  `stop_local_cdp_browser_for` is invoked per-worktree from three call sites
  (`routes/cdp_browser.rs:92`, `routes/worktrees.rs:444`, `:624`). With one
  browser process per project, a per-worktree stop signal must be reconciled
  against the project's *other* live panes (refcount / attach-count), or
  worktree A's close kills worktree B's browser mid-screencast.
- **Agent-side callers have no repoId:** the MCP/agent path sends a bare
  worktree path (the reason `canonical_worktree_key` `cdp_browser.rs:271–277`
  exists). Project keying needs bare-path→project resolution (worktree
  registry lookup; `git rev-parse --git-common-dir` fallback).
  `github-pr:repo:42` pseudo-worktrees (test `cdp_browser.rs:824`) have no
  repo root — they keep per-key behavior.
- **`AGENTUM_BROWSER_PER_WORKTREE=0` opt-out (`cdp_browser.rs:340–342`):**
  define its post-change meaning (suggested: unchanged — everything shares the
  one root browser).
- **Behavior change, on purpose:** two worktrees of one project will now share
  logins (today they're isolated from each other). This matches the goal
  ("per project") but inverts the v0.27-era per-worktree isolation memory —
  call it out in the changelog/issue.
- **Disk growth:** profiles are no longer deleted on worktree close, so they
  accumulate per project. Retention needs an answer (O3) — at minimum the
  explicit clear action; the orphan reap must NOT become a profile-deleter.
- **pkill safety invariant:** every profile path must stay under the
  `cdp-browser` state dir so `pkill_by_signature` can never match the user's
  real Chrome (`cdp_browser.rs:410` comment).
- **Server is API-only** — profile state lives under
  `agentum_store::paths::state_dir()`, no UI assets in the server crate.
- **Screencast is the browser** — the native WKWebView surface is secondary;
  don't let re-keying change which surface renders by default.
- **Tests touching user paths** isolate via `AGENTUM_HOME` (temp dir), never
  `XDG_*` (macOS `directories` gotcha).

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `per-project-cdp-profile` — server-side project keying + one-browser-per-
     project registry + stop-without-delete (AC 1, 2, 6).
  2. `plain-workspace-and-native-routing` — non-worktree routing + WKWebView
     data-store re-key (AC 3, 4).
  3. `clear-browser-data-action` — real command/route + UI affordance +
     scoped delete (AC 5).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` (new
  key-derivation + stop-without-delete tests green) and
  `npm run build --prefix crates/agentum-desktop/ui` + vitest on
  `browser-pane` (new routing test green).
- **`qa.sh` asserts (browser QA):** in workspace A (project P) open a browser
  pane, set a cookie/login on a test page, close the tab, reopen → state
  survives; relaunch app → state survives; open project Q's browser → state
  absent; invoke "Clear browser data" on P → P's state gone, Q's intact.

## Decisions (PM-locked, 2026-07-09)

- **D1 — Key by PROJECT.** Worktrees of one repo share logins; different
  repos never do. The account boundary is the project (one staging env per
  repo); persistent per-worktree profiles would re-demand a login per
  ephemeral worktree. Deliberate behavior change vs v0.27 isolation — flagged
  in Risks + changelog.
- **D2 — Project identity = registry `Repo.id` UUID** (`routes/repos.rs:140`,
  persisted `~/.agentum/repos.json`, idempotent-by-path `:126–128`; carried on
  every worktree row as `repo_id`, `routes/worktrees.rs:50`, and in the UI
  worktree id `<repoId>::<path>`, `:371`). Stable across worktree add/remove
  and relaunch; filesystem/tmux-safe (no lossy sanitization). Caveat accepted:
  repo remove+re-add mints a new UUID → old profile orphaned = "new project".
  Bare-path (agent-side) resolution chain = architect.
- **D3 — Retention: the explicit clear action is the ONLY deleter.** Orphan
  reap stays a process-killer, never a dir-deleter; repo removal does not
  cascade-delete. Revisit only if disk growth bites.
- **D4 — Migration: one-time boot sweep deletes legacy per-worktree dirs**
  under `cdp-browser/` (today's contract already deletes them constantly —
  nothing durable is lost). New dirs use the `project-` prefix so the sweep is
  unambiguous; never touches the shared root dir or temp test profiles.

## Open questions

None requiring a human. O1–O4 are locked above (D1–D4). Remaining
architect-level research (not blockers): (a) whether Tauri exposes WKWebView
data-store removal by identifier (AC 5 fallback defined); (b) the exact
bare-path→project resolution fallback chain (D2); (c) the project-scoped
teardown mechanism (refcount vs attach-registry) for AC 2.
