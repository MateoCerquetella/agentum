# Architecture — Spec 014: Per-project persistent browser profiles

- **Spec:** 014-per-project-browser-profiles
- **Date:** 2026-07-09
- **Author:** Architect (autonomous /sdd-loop iteration 2)
- **Inputs:** `spec.md` (PM-gated, D1–D4 locked), `handoffs/01-pm-to-architect.md`; PM findings 1–7 all addressed.
- **Native-half research result (Decision D):** wry 0.55.1 / tauri 2.11.2 expose **no** remove-data-store-by-identifier API, but `tauri::webview::Webview::clear_all_browsing_data()` exists (backed by wry `WebView::clear_all_browsing_data`, verified on docs.rs for both crates). The native clear therefore works **through a live webview on the project's store**; with no live webview it degrades to the AC 5 observable warning. No silent success anywhere.

---

## 0. Design overview

Replace the browser registry's per-worktree key with a resolved **`BrowserScope`** — `Shared` (truly contextless), `Project { repo_id }` (the registry `Repo.id` UUID, D2), or `Adhoc { key }` (pseudo-worktrees like `github-pr:repo:42`, preserving today's behavior) — derived from whatever callers already send (`<repoId>::<path>` pane ids, bare agent paths, or a bare repoId). One Chromium process/port/tmux/profile per project, profile dir `state_dir()/cdp-browser/project-<repoId>/`, guaranteed single-process-per-dir by the existing idempotent registry (`cdp_browser.rs:255-258`, `:331-394`) — only the key changes. Teardown splits "stop the process" from "delete the profile": the three per-worktree stop triggers become a **release** that stops the project browser only when a server-side attach refcount (ground truth: live screencast WebSockets) is zero, and **never** deletes a project profile dir — the new clear-browser-data action is the only deleter (D3). The shared contextless profile relocates to `cdp-browser/shared/` so its existing delete-on-stop can no longer nuke nested project dirs, and a boot sweep (run right after `reap_orphaned_cdp_browsers`) deletes every top-level entry under `cdp-browser/` that is neither `project-*` nor `shared` (D4). The native WKWebView surface re-keys its data-store id to the same project identity, and one UI affordance (a menu on the screencast pane toolbar) drives the end-to-end clear.

---

## 1. Decisions

### A — Registry key: a resolved `BrowserScope`, registry keyed by the profile token

**Chosen.** In `crates/agentum-server/src/cdp_browser.rs`:

```rust
/// Which browser a raw caller-supplied context resolves to.
pub(crate) enum BrowserScope {
    /// Truly project-less (empty context) → the shared root browser.
    Shared,
    /// A registered project (registry Repo.id UUID, spec 014 D2).
    Project { repo_id: String },
    /// A pseudo-worktree with no repo root (e.g. `github-pr:repo:42`) or an
    /// unresolvable bare path — keeps today's per-key isolated, ephemeral behavior.
    Adhoc { key: String },
}

impl BrowserScope {
    /// Filesystem/tmux-safe token; also the registry + attach-count key.
    /// None for Shared. Project => "project-<sanitized repo_id>" (the prefix is
    /// applied AFTER sanitization so it can never be truncated away by the
    /// 48-char tail bound in sanitize_worktree_token, cdp_browser.rs:292-298;
    /// a UUID passes sanitization unchanged, so the dir is literally
    /// `project-<uuid>` — matching AC 1).
    fn profile_token(&self) -> Option<String>;
}
```

- The in-memory registry (`worktree_registry`, `cdp_browser.rs:255-258`) keeps its shape (`HashMap<String, WorktreeBrowser>`, std mutex never held across `.await`) but is renamed `browser_registry` and **keyed by the profile token**, not the raw canonical key. This is a deliberate simplification with a bug fix inside: today two distinct paths whose sanitized 48-char tails collide get two registry entries/ports but ONE profile dir (`worktree_profile_dir` `:307`) — a latent Chromium single-instance-lock conflict. Token-keyed, a collision collapses to one browser, which Chromium requires.
- `ensure_local_cdp_browser_for` (`:331`) resolves the scope first: `Shared` → the existing shared path; `Project`/`Adhoc` → the existing per-key launch machinery (`launch_lock`, leftover-grace, `build_chrome_argv` `:580`) verbatim, with tmux `{CDP_TMUX_TARGET}-{token}` and profile `cdp-browser/<token>`. Exactly one Chromium per project falls out of the existing idempotent ensure (PM finding 2) — no new process management.
- **`canonical_worktree_key` (`:271`) is retained**, demoted from "the registry key" to the path-extraction helper inside scope resolution (strip `<repoId>::` prefix and `::workspace:<uuid>` suffix before path→repo lookup). Its test (`:804-827`, incl. the github-pr pseudo-key case at `:824`) stays green unchanged.

**Rejected:** a canonical string scheme (`"project:<id>"` strings everywhere) without an enum — stringly-typed scope decisions leak into every call site and are untestable as a unit; the enum makes the resolution chain (Decision C) a pure, exhaustively-tested function. Also rejected: keying the registry by `repo_id` raw — then Adhoc and Project need two registries or a prefixing convention anyway; the token already is that convention.

### B — Teardown: server-side attach refcount, ground truth = live screencast WebSockets

**Chosen.** The screencast WS is the only faithful signal for "a pane is attached" — the route (`routes/cdp_screencast.rs:83-124`) currently tracks nothing. Add to `cdp_browser.rs`:

```rust
/// token → live screencast attach count. Std mutex; guard Drop decrements.
fn attach_counts() -> &'static std::sync::Mutex<HashMap<String, usize>>;
pub(crate) struct BrowserAttachGuard(Option<String>); // Drop => decrement
/// Registered only for Project scopes (Adhoc keeps kill-always; Shared's stop
/// is explicit-only). Returns an inert guard for non-project scopes.
pub(crate) fn register_browser_attach(raw: &str) -> BrowserAttachGuard;
```

`routes/cdp_screencast.rs::screencast` creates the guard when it resolves via `worktreeId` (`:100-106`) and **moves it into the `run()` future** (`:123`), so the count decrements exactly when the WS task ends — close, drop, or panic.

`stop_local_cdp_browser_for` (`cdp_browser.rs:450-469`) becomes a scope-aware **release**:

- **Project:** if `attach_counts[token] > 0` → complete no-op (worktree A's close cannot kill the browser worktree B is screencasting — AC 2). If 0 → kill tmux (registry entry), `pkill_by_signature(profile_path)` (`:416`, carrying over the kill-even-without-registry-entry hygiene of `:461-465`), remove the registry entry, and **never** `remove_dir_all` (the `:466` delete is gone for this arm — the heart of the spec).
- **Adhoc:** today's behavior verbatim — kill + pkill + `remove_dir_all` (pseudo-keys keep per-key ephemerality, PM finding 3).
- **Shared** (empty key): no-op. (Today an empty key sanitizes to `"wt"` and deletes `cdp-browser/wt` — a latent oddity this removes; the shared browser has its own explicit stop route.)

The three callers: last-tab-close (`routes/cdp_browser.rs:92`) already sends the full `<repoId>::<path>` id — unchanged wire. Worktree remove (`routes/worktrees.rs:444`) and prune (`:624`) currently pass the **bare path**, which at removal time no longer resolves (the row was deregistered at `:437-439` and the dir git-removed) — change them to pass the full id (`body.worktree_id` at `:410`; `format!("{repo_id}::{}", wt.path)` in prune, both already in scope) so they resolve to `Project` and take the idle-check release.

Accepted benign races (both self-healing, both allowed by AC 2's "process may stay alive longer"): (1) the UI fires the stop HTTP before/after its own WS closes (`ui/src/store/slices/browser.ts:781-783`) — worst case the process lingers until the boot reap; (2) a stop racing a fresh attach kills a just-attached pane's browser — the client's reconnect (`cdp-screencast-client.ts:119-131`) re-ensures and relaunches on the same profile.

**Rejected:** (a) *stop only on app quit + explicit clear* (no counting) — simplest, but resurrects the "dozens of leftover Chrome processes" pile-up the reap was built for (`cdp_browser.rs:431-437`); with many projects open over a long session that is a real memory regression. (b) *UI-computed project-wide tab count* (the store has `unifiedTabsByWorktree`) — puts a process-lifecycle invariant in one client; breaks the moment a second client (MCP agent, future TUI) attaches; the server must not trust clients for kill decisions. The refcount is ~40 lines and keeps eager cleanup in the common case.

### C — Bare-context → project resolution chain

**Chosen.** `resolve_browser_scope(raw: &str) -> BrowserScope` (async; the git probe needs a subprocess), built on a **pure core** `resolve_scope_from_tables(raw, worktree_ids: &[String], repos: &[(id, path)])` so the chain is unit-testable without touching `~/.agentum/*.json` (those live under `HOME`, not `AGENTUM_HOME` — `routes/worktrees.rs:66`, `routes/repos.rs:76` — so injection, not env mutation, is the test seam). Order:

1. Trim; **empty → `Shared`**.
2. **Contains `::`** → `<repoId>::<path>` pane id (`routes/worktrees.rs:371`; folder projects append `::workspace:<uuid>`) → `Project { repo_id: prefix }`. Trust the prefix without registry validation — the UUID *is* the identity (D2), validation adds a file read and a failure mode, and a stale-but-real repoId should still hit its profile. Folder-project workspace instances of one repo therefore share one browser — consistent with D1.
3. **Exact match against a registered `Repo.id`** (raw is a bare UUID, no `::`) → `Project { repo_id: raw }`. This is the cheap hook for plain-workspace / project-hub surfaces that have a repo but no path (AC 3); a filesystem path can never collide with a UUID.
4. **Bare path** (the agent/MCP side — `mcp_url_with_worktree` tags the *effective work path*, `routes/sessions/provision.rs:81-96`; consumed at `routes/mcp.rs:893`):
   a. `worktrees.json` row whose id's path part equals the path → its `repo_id` (`routes/worktrees.rs:50`).
   b. `repos.json` repo whose `path` equals the path (a session in the main checkout) → its `id` (`routes/repos.rs:126-128`).
   c. `git -C <path> rev-parse --git-common-dir` → main repo root → match `repos.json` by path → its `id`. (tokio `Command`; a dead/non-git path just fails this probe.)
   d. **Miss → `Adhoc { key: path }`** — byte-for-byte today's behavior (isolated per-key browser), NOT the shared root. Dumping unknown contexts into the shared profile is exactly the cross-context cookie leak the spec kills.
5. **Anything else** (e.g. `github-pr:repo:42`) → `Adhoc { key: raw }`.

**Rejected:** validating the `::` prefix against `repos.json` (step 2) — see above; and resolving paths via `canonicalize()` — the stored registry paths are what callers echo back, string equality suffices, and canonicalization introduces symlink/TCC surprises on macOS.

### D — Clear-browser-data end-to-end

**Chosen.**

- **Server** (route module: `routes/cdp_browser.rs` — the existing browser-lifecycle surface, matching its `stop-worktree` convention): `POST /api/cdp-browser/clear-project-data`, body `{repoId}`, authed like the rest of the router (no `is_public` change). Handler calls new `cdp_browser::clear_project_browser_data(repo_id)`: force-stop (kill tmux + `pkill_by_signature(project_dir)`, **ignoring** attach counts — a clear is explicit) then `remove_dir_all(cdp-browser/project-<token>)`, propagating errors. Response `{ok, clearedCdp, warnings: []}`. A live screencast pane on that project sees its WS die and reconnects → relaunches on a fresh empty profile (correct post-clear UX).
- **Native** (desktop shell): new `#[tauri::command] browser_clear_project_data(app, repo_id: String) -> NativeClearResult { cleared: bool, warning: Option<String> }` — **flat named params** per the repo's hard Tauri rule. Mechanism: wry/tauri expose no `WKWebsiteDataStore` remove-by-identifier (verified, docs.rs wry 0.55.1 + tauri 2.11.2), but `Webview::clear_all_browsing_data()` exists. So: track store tokens at webview creation (a managed `NativeStoreRegistry: Mutex<HashMap<String /*label*/, String /*store token*/>>` populated in `browser_webview_open`, `browser_native.rs:131-245`; stale labels filtered by `app.get_webview()` liveness), find any live browser-page webview on the project's store, call `clear_all_browsing_data()` on it (one call clears the shared store), return `cleared: true`. No live webview → `cleared: false, warning: "native store not cleared — no live native tab for this project"` (the AC 5 degradation, observable by construction). The stub `browser_session_delete_profile` (`commands/browser.rs:37`) **flips from hardcoded `true` to `false`** — the silent-success failure mode is removed; the legacy profiles UI will now show an honest failure instead of a fake success (flagged in Risks).
- **UI affordance (ONE surface):** a small `…` DropdownMenu added to the **screencast pane toolbar** (`AgentBrowserScreencastPane.tsx:435-479` — currently back/forward/reload/address only) with a single item "Clear browsing data for this project…" + a confirm `Dialog` (pattern: `BrowserToolbarMenu.tsx:303-331`). Rendered only when a project can be derived from `worktreeId`. Handler: derive `repoId` (pure helper, Decision C step 2/3 mirrored in TS), call the server route (new fn in `runtime/cdp-screencast-client.ts`, same shape as `stopWorktreeCdpBrowser` `:287-302`), then the native command; toast the aggregate, surfacing any `warning` verbatim. The screencast pane is the right single surface: it is the default browser ("screencast IS the browser"), it is where the persistent login lives, and it already carries `worktreeId`.

**Rejected:** a Settings-page per-project list (`settings/BrowserPane.tsx`) — requires enumerating repos with per-row buttons, a bigger surface for the same AC; and routing the native clear through the server's desktop bridge (`routes/mcp.rs:872`) — the UI orchestrating two calls is simpler than adding a bridge op, and the server half must work on a bridgeless daemon anyway.

### E — D4 boot sweep

**Chosen.** `pub async fn sweep_legacy_profile_dirs()` in `cdp_browser.rs`, called from the desktop boot task at `agentum-desktop/src/lib.rs:129-131` **sequentially after** `reap_orphaned_cdp_browsers().await` (kill first, then delete — never rip a profile out from under a live process; the reap just killed every agentum Chromium, so nothing under `cdp-browser/` is in use at sweep time). Logic: for each **top-level** entry of `state_dir()/cdp-browser/`, delete it (dir or file) unless its file name is `shared` or starts with `project-`. Runs **every boot** (idempotent — second boot finds nothing legacy), not marker-gated: this also keeps Adhoc/pseudo-key dirs boot-ephemeral, matching their today's-contract lifecycle. It never recurses into `project-*` or `shared`, never touches anything outside `cdp-browser/`, and never sees the hermetic self-test's profile (that lives under `std::env::temp_dir()`, `cdp_driver.rs:1302`).

Why deleting root-level **files** too is correct: after Decision G relocates the shared profile, any files directly under `cdp-browser/` (`Local State`, `First Run`, …) are the *old* shared profile's Chromium internals — ephemeral by today's contract (`stop_local_cdp_browser` deleted them on every stop). Without the relocation, the sweep is **impossible to make safe**: legacy per-worktree tokens (`[A-Za-z0-9-_]`, `:280-304`) are indistinguishable by name from Chromium-internal dirs like `Default` sitting in the same parent.

**Rejected:** a one-time marker-file gate — adds state for no benefit and forfeits the Adhoc backstop; a name-heuristic sweep without the shared relocation — unsafe as shown above.

### F — `AGENTUM_BROWSER_PER_WORKTREE` post-change semantics

**Chosen (PM suggestion adopted):** unchanged meaning, name kept for compat — `=0` opts out of keyed isolation entirely; every context (project, adhoc, plain) shares the one root browser (`cdp_browser.rs:340-343` moves after scope resolution but keeps the same effect). Documented consequence: with the opt-out, per-project persistence is off (the shared profile keeps its delete-on-stop). **Rejected:** renaming to `AGENTUM_BROWSER_PER_PROJECT` — breaks existing users' env for a cosmetic gain; a doc comment notes the historical name.

### G — `stop_local_cdp_browser`'s shared-dir delete

**Chosen:** the delete **stays** for the contextless browser, made safe by relocation: `user_data_dir()` (`cdp_browser.rs:774-781`) becomes `state_dir()/cdp-browser/shared`, so the existing `remove_dir_all(user_data_dir())` at `:218-221` now deletes only the shared profile — it is structurally incapable of touching `project-*` siblings. The contextless profile keeps today's ephemeral semantics (AC 3 keeps it "only for truly project-less contexts"). One-time cost: existing users' shared-profile state is lost at first relaunch — it was already deleted on every shared stop, so nothing durable is lost. `pkill_by_signature` and the reap keep matching (the path still contains `/cdp-browser/`). The remote host path (`$HOME/.agentum/cdp-browser`, `:493`) is a different machine and out of scope — untouched.

**Rejected:** dropping the delete — an un-clearable, cross-project-leaking contextless cookie store is the exact privacy failure this spec exists to remove; and keeping the shared profile at the root with a "delete children selectively" stop — every future edit to that code risks the project profiles.

### H — Native surface keying

**Chosen.** `browser_native.rs`: new pure fn `project_store_token(worktree_id: &str) -> String` — if the id contains `::`, return `format!("project-{prefix}")`; else if it parses as a bare UUID, `format!("project-{raw}")`; else return the raw id (per-key fallback, mirroring Adhoc). `worktree_data_store_id` (`:48`) hashes this token instead of the raw id (same SHA-256→16-byte scheme, `:48-54`). The `project-` domain-prefix guarantees the new ids can never collide with any legacy per-worktree store id (legacy inputs were raw ids/paths, never `project-*`). The native command learns the project from the **same `worktree_id` the UI already passes** (`browser_webview_open`, `:135`; caller `NativeBrowserPagePane.tsx:147`) — no signature change, no new plumbing. Legacy WKWebView stores are orphaned on disk (accepted; same class as D2's repo-re-add caveat — WebKit owns that storage and no removal API exists; noted in Risks). The same fn feeds the `NativeStoreRegistry` in Decision D.

**Rejected:** adding an explicit `project_id` command parameter — redundant with the prefix the UI already sends, and every call site would need touching for zero information gain.

---

## 2. Data / control flow

### Open-browser-pane (screencast, the default surface)

```
UI pane (worktreeId = "<repoId>::<path>")
  → WS /api/cdp-browser/screencast?worktreeId=…            (cdp-screencast-client.ts:88-90)
  → screencast handler (routes/cdp_screencast.rs:83)
      resolve = ensure_local_cdp_browser_for(wt)           (:104)
        → resolve_browser_scope(wt) → Project{repo_id}     (Decision C, step 2)
        → token = "project-<repoId>"
        → browser_registry[token] listening? reuse port
          : launch_lock → build_chrome_argv(exe, port,
              state_dir()/cdp-browser/project-<repoId>)    (cdp_browser.rs:580)
            tmux "agentum-cdp-browser-project-<repoId>" → register
      guard = register_browser_attach(wt)                  (count[token] += 1)
      set_foreground_cdp_port(port)                        (:119-121, unchanged)
  → run(socket, …, guard)  — frames stream; guard lives with the WS task
Pane close → WS ends → guard Drop (count -= 1)
UI fires POST /api/cdp-browser/stop-worktree {worktreeId}  (browser.ts:782)
  → release: count[token] > 0 ? no-op
    : kill tmux + pkill(profile path) + registry.remove — dir KEPT (AC 2)
Agent op: MCP /mcp?worktree=<bare path> (provision.rs:81) → ensure_for
  → chain step 4a (worktrees.json) → SAME token/port — agent drives what the user sees.
Relaunch: boot reap kills orphans, sweep spares project-* → next ensure reuses the
  dir → login survives (AC 2); worktree remove/prune pass the full id → same release.
```

### Clear-browser-data

```
Screencast toolbar "…" → "Clear browsing data for this project…" → confirm Dialog
  repoId = deriveProjectRepoId(worktreeId)                 (pure TS, vitest'd)
  1) POST /api/cdp-browser/clear-project-data {repoId}
       → clear_project_browser_data: kill tmux + pkill(project dir sig)
         (attach counts IGNORED — explicit clear) → remove_dir_all(project-<id>)
       → {ok, clearedCdp, warnings}
  2) invoke browser_clear_project_data(repoId)             (flat args)
       → NativeStoreRegistry: live webview on token "project-<repoId>"?
         → webview.clear_all_browsing_data() → {cleared:true}
         : {cleared:false, warning:"native store not cleared — no live native tab"}
  3) toast aggregate; any warning shown verbatim (AC 5 — never silent)
Live panes on that project: WS dies → client reconnects (cdp-screencast-client.ts:119)
  → ensure relaunches on a fresh, empty project profile. Other projects untouched.
```

---

## 3. Test strategy (I)

All Rust tests must keep `cargo test --workspace --lib` green with no tmux/Chrome dependency (tmux calls in the exercised paths are `let _`-ignored; `pkill -f` in tests only ever receives paths under a temp `AGENTUM_HOME`, which cannot match a real process). Env-mutating tests take `crate::TEST_ENV_LOCK` (`agentum-server/src/lib.rs:66`) and isolate via `AGENTUM_HOME` (never `XDG_*` — `paths.rs:7-11`), following the `routes/profiles.rs:154-177` pattern including the escape assertion.

**`crates/agentum-server/src/cdp_browser.rs` (`#[cfg(test)]`):**
- `scope_pane_id_and_agent_path_resolve_to_same_project` — `"<uuid>::<path>"` and the bare path (via injected worktree-row fixture) → identical `Project`/token. The spiritual successor of the `:804` contract test.
- `scope_bare_repo_id_workspace_suffix_repo_main_path` — chain steps 2/3/4b, incl. `repo::/folder::workspace:<uuid>` → `Project`.
- `scope_miss_is_adhoc_never_shared` + `scope_github_pr_pseudo_key_is_adhoc` + `scope_empty_is_shared`.
- `scope_git_common_dir_fallback` — temp `git init` repo + `git worktree add`, injected repo table pointing at the main root (the only test that shells to git).
- `project_profile_token_is_prefixed_fs_safe_and_uncollidable` — prefix applied post-sanitize; UUID passes through verbatim.
- `stop_project_scope_never_deletes_profile_dir` — temp `AGENTUM_HOME`, seed `cdp-browser/project-x` + a registry entry on an unused port; release with count 0 → registry entry gone, **dir exists**. (The AC 7-mandated no-delete-on-stop test.)
- `stop_adhoc_scope_deletes_profile_dir` — parity with today's `:466`.
- `release_is_noop_while_project_attached` — hold a `BrowserAttachGuard`; release → registry entry retained, dir retained; drop guard; release → entry removed, dir retained.
- `sweep_deletes_only_legacy_entries` — seed `cdp-browser/{old-worktree-token/, Default/, Local State, project-a/, shared/}` → sweep → only `project-a` + `shared` survive; second sweep is a no-op (idempotency).
- `shared_user_data_dir_is_nested_shared_subdir` — Decision G pin.
- `clear_project_browser_data_deletes_only_that_project` — two `project-*` dirs; clear one; the other intact (the qa "P gone, Q intact" assertion, unit-shaped).
- Keep green unchanged: `canonical_worktree_key_unifies_pane_id_and_agent_path` (`:804`), `chrome_argv_…` (`:830`).

**`crates/agentum-desktop/src/commands/browser_native.rs` (`#[cfg(test)]`):** `project_store_token_prefix_uuid_and_fallback`; `project_store_ids_stable_and_distinct_per_repo`; `project_store_id_never_equals_legacy_worktree_id` (domain-prefix pin). Pure — no `ENV_HOME_TEST_LOCK` needed.

**UI (vitest, pure logic, no jsdom — colocated like `browser-pane/*.test.ts`):** `ui/src/lib/browser-project.test.ts` — `deriveProjectRepoId`: `"<uuid>::<path>"` → uuid; bare uuid → uuid; `github-pr:repo:42` → null; empty → null (the AC 7 "workspace → project profile key" derivation test). Existing `browser-pane` tests stay green. Build gate: `npm run build --prefix crates/agentum-desktop/ui`.

**Untouched-by-construction:** the hermetic CDP self-test (`cdp_driver.rs:1286-1310`, `#[ignore]`, temp-dir profile) — no code it exercises changes its inputs. The `#[ignore]` live QA path is `qa.sh` per the spec's harness section.

---

## 4. Build order (J) — smallest-first, each step independently green

Gate command shorthand: **R** = `cargo test -p agentum-server --lib` (steps touching desktop Rust add `cargo test -p agentum-desktop --lib && cargo build -p agentum-desktop`), **U** = `npm run build --prefix crates/agentum-desktop/ui` + vitest.

**Feature 1 — `per-project-cdp-profile` (AC 1, 2, 6):**
1. `BrowserScope` + `resolve_scope_from_tables` (pure) + `resolve_browser_scope` (json loaders + git probe) + all scope tests. No behavior change yet (nothing calls it). **R**
2. Relocate the shared profile: `user_data_dir()` → `…/cdp-browser/shared` + pin test. (`stop_local_cdp_browser` needs zero edits — Decision G.) **R**
3. Re-key the launch path: `ensure_local_cdp_browser_for` resolves the scope; registry renamed `browser_registry`, keyed by token; env opt-out check moves after resolution (same effect). **R**
4. Split stop from delete: scope-aware release + `attach_counts`/`BrowserAttachGuard` + guard wired into `routes/cdp_screencast.rs::screencast→run`; worktree remove/prune callers pass the full `<repoId>::<path>` id. Tests: no-delete, adhoc-delete, attached-no-op. **R** *(This step lands AC 2 — the riskiest change; build it after 1–3 so the scope/token layer is already pinned.)*
5. `sweep_legacy_profile_dirs` + wire after the reap in `agentum-desktop/src/lib.rs:129-131` + sweep tests. **R + desktop build**

**Feature 2 — `plain-workspace-and-native-routing` (AC 3, 4):**
6. UI: `deriveProjectRepoId` helper + verify/fix that plain-workspace and project-hub browser surfaces pass a project-scoped id (`<repoId>::<path>` where a path exists, bare `repoId` where not — the server accepts both from step 1). Vitest. **U**
7. Native re-key: `project_store_token` feeding `worktree_data_store_id` (`browser_native.rs:48`, `:158-168`) + desktop unit tests. **R(desktop) + U**

**Feature 3 — `clear-browser-data-action` (AC 5):**
8. Server: `clear_project_browser_data` + `POST /api/cdp-browser/clear-project-data` in `routes/cdp_browser.rs` + only-mine test. **R**
9. Native: `NativeStoreRegistry` (populate in `browser_webview_open`) + `browser_clear_project_data` command (flat args, registered in `lib.rs`'s `generate_handler!`) + flip the `browser_session_delete_profile` stub to `false`. **R(desktop)**
10. UI: screencast-toolbar menu + confirm dialog + `clearProjectBrowserData` runtime fn + aggregate toast (warnings verbatim). **U**

---

## 5. Risks the developer must watch

- **The std registry/count mutexes must never be held across `.await`** — the existing registry comment (`cdp_browser.rs:253-254`) is the law; the guard's Drop decrement is sync and safe.
- **The attach guard must live inside `run()`'s future**, not the `screencast` handler scope — dropped-at-handler-return silently zeroes the count and resurrects the AC 2 bug.
- **Sweep strictly after reap** in the same task — reordering deletes profiles under live processes.
- **`pkill` in tests:** every asserted path must sit under the temp `AGENTUM_HOME` (assert it, `profiles.rs:171-177` style) so a test can never signature-match a real process.
- **Plain-workspace pane ids need runtime verification** (step 6): if any surface passes `worktreeId: undefined` where a project exists, it silently lands on the shared profile — AC 3's qa case catches it, but check the tab store's id at the hub/plain surfaces first.
- **Paneless-agent edge (accepted):** a worktree removal while another workspace's *paneless* agent drives the project browser kills it mid-op; every MCP browser op re-ensures (`routes/mcp.rs:893`) so it relaunches with the profile intact — open tabs are lost, ops fail loud. Rare; do not add op-scoped refcounting for it.
- **Legacy profiles UI honesty:** the stub flip (step 9) makes `BrowserProfileRow` deletes show a real failure where they faked success — expected, per AC 5's rationale; note it in the PR.
- **Deliberate behavior change (changelog):** two worktrees of one project now share logins (D1, inverts v0.27-era isolation); orphaned legacy WKWebView stores and repo-re-add profile orphans (D2 caveat) are accepted, not bugs.
- **`clear_all_browsing_data` on macOS:** verified present in tauri 2.11/wry 0.55 API docs; if the runtime errors on WKWebView, return it as the AC 5 warning — never swallow.

## 6. Sacred / untouched

- `pkill_by_signature` (`cdp_browser.rs:416-429`) matches only absolute paths under our `cdp-browser` state dir — every new profile path (shared/, project-*, adhoc) stays under it.
- The hermetic CDP self-test (`cdp_driver.rs:1286-1310`): temp profile, own port — untouched and unreachable by the sweep.
- `reap_orphaned_cdp_browsers` (`:438-445`): stays a process-killer + registry-clear; deletes **nothing**; neither reap nor sweep ever removes `project-*` or `shared` (D3: the clear action is the only project-profile deleter).
- Push-based streaming: no polling added anywhere; the attach count is event-driven (WS lifecycle).
- `agentum-server` stays API-only; all state under `agentum_store::paths::state_dir()`.
- The remote SSH browser path (`remote_chrome_launch_script`, `:488-503`) and `ensure_remote_cdp_browser` — out of scope, byte-identical.
- `spawn_agent_into_pane`, YOLO translation, MCP wiring (`provision.rs`) — only the already-existing `?worktree=` tag is consumed differently server-side.
- Screencast-vs-native default: CDP screencast remains the browser; re-keying changes no surface selection.
- `canonical_worktree_key` and its `:804` contract test — retained, green.

---

## 7. Design notes (architect META)

- **Spec precision notes (no contradictions):** (1) AC 2's "stopped only when no workspace … has a browser pane attached" is implemented as the WS refcount *plus* explicitly-allowed lingering (a process may outlive its last pane until quit/boot reap when a release race goes the other way) — inside AC 2's stated tolerance; killing eagerly is NOT an AC. (2) The spec/handoff call the D4 sweep "one-time"; it is designed every-boot-idempotent because that is strictly safer, simpler (no marker state), and gives Adhoc dirs their today's-contract ephemerality — it deletes nothing a one-time sweep wouldn't.
- **Needs-human:** none. The one researchable unknown (native data-store removal API) was resolved: unavailable by identifier in wry 0.55.1/tauri 2.11.2; `clear_all_browsing_data` via a live webview is the mechanism, AC 5's degradation covers the no-live-webview case.
- **Confidence:** A high · B high · C high · D medium-high (macOS runtime behavior of `clear_all_browsing_data` verified in API docs, not executed; degradation bounds the downside) · E high · F high · G high · H high · I high · J high. Weakest empirical link: which id plain-workspace/hub browser tabs currently carry (build-order step 6 verifies at build time; the design accepts both forms either way).
