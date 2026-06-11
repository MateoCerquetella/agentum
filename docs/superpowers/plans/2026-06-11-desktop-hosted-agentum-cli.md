# Desktop-Hosted agentum CLI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the three capability cards (Agent Browser Use, Computer Use, Agent Orchestration) actually work by routing their skill command surface through the desktop's embedded `agentum-server`, driven by a single `agentum` binary — no second binary, no standalone reimplementation.

**Architecture:** The desktop boots `agentum-server` in-process (`serve_embedded_loopback`). Today nothing publishes that ephemeral port, so a CLI run inside a desktop-managed pane can't find it. We (1) publish the server URL into every pane's environment, (2) turn the missing skill commands into thin HTTP clients of that server, and (3) add a `DesktopBridge` trait to `AppState` so browser/computer routes can reach the Tauri `AppHandle` (drive native webviews, run the macOS Accessibility engine). When the server runs standalone (`agentum serve`, no desktop), the bridge is absent and those routes return `501 Not Implemented` — honest, not a crash. Orchestration/worktree/status work everywhere (incl. SSH hosts via existing host routing); browser/computer are local-desktop-only by nature.

**Tech Stack:** Rust (axum, clap, reqwest, tokio), Tauri 2 (`AppHandle`, `Webview::eval`), macOS Accessibility (`AXUIElement` via `accessibility`/`core-foundation` crates), SQLite (sqlx) for orchestration tasks.

---

## The DesktopBridge abstraction (read first — phases 3 & 4 depend on it)

`AppState` gains one optional field:

```rust
// crates/agentum-server/src/lib.rs
pub trait DesktopBridge: Send + Sync {
    /// Drive a native browser webview. Returns JSON the route forwards verbatim.
    fn browser(&self, op: BrowserOp) -> futures_core::future::BoxFuture<'_, anyhow::Result<serde_json::Value>>;
    /// Drive the macOS Accessibility engine.
    fn computer(&self, op: ComputerOp) -> futures_core::future::BoxFuture<'_, anyhow::Result<serde_json::Value>>;
}

pub struct AppState {
    // ...existing fields...
    pub api_base_url: Option<String>,        // Phase 1: self-URL injected into panes
    pub desktop_bridge: Option<std::sync::Arc<dyn DesktopBridge>>, // Phases 3-4
}
```

- `serve_embedded_loopback` keeps its signature; a new `serve_embedded_loopback_with_bridge(store, bridge)` sets `desktop_bridge = Some(bridge)` and `api_base_url = Some(format!("http://{addr}"))`. The desktop calls the `_with_bridge` variant.
- `BrowserOp` / `ComputerOp` are plain enums defined in `agentum-server` (so the crate has no Tauri dependency). The desktop crate implements `DesktopBridge` and translates them to Tauri calls.
- Standalone `serve` leaves `desktop_bridge = None`; `/api/browser/*` and `/api/computer/*` return `501` with body `{"error":"requires the agentum desktop app"}`.

---

## File Structure

**Phase 1 — discovery keystone**
- Modify `crates/agentum-server/src/lib.rs` — add `api_base_url` to `AppState`; set it in `serve_embedded_loopback`.
- Modify `crates/agentum-server/src/routes/sessions.rs:551-593` — inject `AGENTUM_API_URL` into `launch.env`; replace hardcoded `8822` hook URL with `state.api_base_url`.
- Create `crates/agentum-cli/src/api_base.rs` — `resolve_api_base()`: `$AGENTUM_API_URL` → active profile → `127.0.0.1:8822`.

**Phase 2 — thin CLI subcommands**
- Modify `crates/agentum-cli/src/cli.rs` — add `Status`, `Worktree`, `Orchestration`, `Wait`, `Exec` to `Cmd`; dispatch arms.
- Create `crates/agentum-cli/src/commands/status.rs`, `worktree.rs`, `orchestration.rs`, `wait.rs`, `exec.rs`.
- Create `crates/agentum-cli/src/http.rs` — shared thin JSON client built on `resolve_api_base()`.
- Create `crates/agentum-server/src/routes/orchestration.rs` — task DAG endpoints (CRUD over a new `tasks` table); register in `lib.rs::router`.
- Modify `crates/agentum-store/src/lib.rs` + migrations — `tasks` table + repo methods.

**Phase 3 — browser automation**
- Modify `crates/agentum-server/src/lib.rs` — `DesktopBridge` trait, `BrowserOp` enum, `serve_embedded_loopback_with_bridge`.
- Create `crates/agentum-server/src/routes/browser.rs` — `/api/browser/{tabs,snapshot,click,fill,screenshot,navigate}`.
- Create `crates/agentum-desktop/src/bridge.rs` — `impl DesktopBridge` holding `AppHandle`; browser ops via `Webview::eval` + existing `browser_native.rs`.
- Modify `crates/agentum-desktop/src/lib.rs:55` — call `_with_bridge`.
- Create `crates/agentum-cli/src/commands/browser.rs` — `tab`/`snapshot`/`click`/`fill` subcommands.

**Phase 4 — macOS computer-use AX engine**
- Create `crates/agentum-desktop/src/computer/mod.rs`, `ax.rs` (AXUIElement walk), `actions.rs` (click/type/scroll/value).
- Modify `crates/agentum-server/src/lib.rs` — `ComputerOp` enum.
- Create `crates/agentum-server/src/routes/computer.rs` — `/api/computer/{capabilities,permissions,list-apps,get-app-state,click,set-value,type-text,press-key,scroll}`.
- Modify `crates/agentum-desktop/src/bridge.rs` — implement `computer()`.
- Create `crates/agentum-cli/src/commands/computer.rs`.

---

## Phase 1 — Discovery Keystone

### Task 1: `AppState.api_base_url` + embedded server sets it

**Files:**
- Modify: `crates/agentum-server/src/lib.rs:77-115` (struct), `:148-160` (ctors), `:423-448` (`serve_embedded_loopback`)
- Test: `crates/agentum-server/src/lib.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn embedded_loopback_sets_api_base_url() {
    let store = agentum_store::Store::open_in_memory().await.unwrap();
    let addr = serve_embedded_loopback(store).await.unwrap();
    // The state the router was built with must carry its own URL.
    // Exposed via a test hook: serve_embedded_loopback_state(store) -> (addr, AppState)
    let (addr2, state) = serve_embedded_loopback_state(
        agentum_store::Store::open_in_memory().await.unwrap()
    ).await.unwrap();
    assert_eq!(state.api_base_url.as_deref(), Some(format!("http://{addr2}").as_str()));
    assert!(addr.port() != 0);
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p agentum-server embedded_loopback_sets_api_base_url`
Expected: FAIL — `api_base_url` field / `serve_embedded_loopback_state` do not exist.

- [ ] **Step 3: Implement**

Add `pub api_base_url: Option<String>` to `AppState` (default `None` in both ctors). Refactor `serve_embedded_loopback` to bind the listener first, compute `addr`, set `state.api_base_url = Some(format!("http://{addr}"))` BEFORE `router(state)`, then serve. Extract a `serve_embedded_loopback_state` helper returning `(SocketAddr, AppState)` for the test; the public fn wraps it.

- [ ] **Step 4: Run test, verify it passes**

Run: `cargo test -p agentum-server embedded_loopback_sets_api_base_url`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-server/src/lib.rs
git commit -m "feat(server): AppState carries its own loopback URL (api_base_url)"
```

### Task 2: Inject `AGENTUM_API_URL` into pane env; fix hardcoded hook URL

**Files:**
- Modify: `crates/agentum-server/src/routes/sessions.rs:551-593`
- Test: `crates/agentum-server/src/routes/sessions.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing test** — a unit test over a helper `pane_env(api_base: Option<&str>, session_id, hook_token) -> Vec<(String,String)>` extracted from `start`:

```rust
#[test]
fn pane_env_publishes_api_url_and_hook() {
    let env = pane_env(Some("http://127.0.0.1:5544"), test_uuid(), "tok");
    let url = env.iter().find(|(k,_)| k == "AGENTUM_API_URL").map(|(_,v)| v.as_str());
    assert_eq!(url, Some("http://127.0.0.1:5544"));
    // Hook URL is derived from the SAME base, never a hardcoded 8822.
    let hook = env.iter().find(|(k,_)| k == "AGENTUM_HOOK_URL").map(|(_,v)| v.clone());
    assert_eq!(hook.as_deref(), Some(&format!("http://127.0.0.1:5544/api/sessions/{}/hook", test_uuid())[..]));
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p agentum-server pane_env_publishes_api_url_and_hook`
Expected: FAIL — `pane_env` helper does not exist.

- [ ] **Step 3: Implement** — extract `fn pane_env(...)` from the local-host block in `start`; push `("AGENTUM_API_URL", base)` for every local session; derive `AGENTUM_HOOK_URL` from `base` (fall back to `http://127.0.0.1:8822` only when `api_base_url` is `None`, i.e. an older standalone path). Call it from `start`.

- [ ] **Step 4: Run test, verify it passes** — `cargo test -p agentum-server pane_env_publishes_api_url_and_hook` → PASS

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-server/src/routes/sessions.rs
git commit -m "feat(server): publish AGENTUM_API_URL into panes; derive hook URL from it"
```

### Task 3: CLI `resolve_api_base()` prefers `$AGENTUM_API_URL`

**Files:**
- Create: `crates/agentum-cli/src/api_base.rs`
- Modify: `crates/agentum-cli/src/lib.rs` (add `pub mod api_base;`)
- Test: in `api_base.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn env_url_wins_over_default() {
    let got = resolve_api_base_from(Some("http://127.0.0.1:9001".into()), None);
    assert_eq!(got, "http://127.0.0.1:9001");
}
#[test]
fn falls_back_to_default_when_unset() {
    assert_eq!(resolve_api_base_from(None, None), "http://127.0.0.1:8822");
}
#[test]
fn profile_url_used_when_env_absent() {
    assert_eq!(resolve_api_base_from(None, Some("https://vps:8822".into())), "https://vps:8822");
}
```

- [ ] **Step 2: Run** `cargo test -p agentum-cli env_url_wins_over_default` → FAIL (fn missing).

- [ ] **Step 3: Implement** — `resolve_api_base_from(env: Option<String>, profile: Option<String>) -> String` with precedence env → profile → `http://127.0.0.1:8822`; public `resolve_api_base()` reads `std::env::var("AGENTUM_API_URL").ok()` and the active profile URL.

- [ ] **Step 4: Run** the three tests → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-cli/src/api_base.rs crates/agentum-cli/src/lib.rs
git commit -m "feat(cli): resolve_api_base prefers AGENTUM_API_URL (desktop discovery)"
```

**Phase 1 exit criteria:** launch the desktop, open a worktree terminal, run `echo $AGENTUM_API_URL` — it prints the embedded server's `http://127.0.0.1:<port>`. `curl $AGENTUM_API_URL/api/health` returns ok.

---

## Phase 2 — Thin CLI Subcommands

### Task 4: Shared thin HTTP client

**Files:** Create `crates/agentum-cli/src/http.rs`; modify `lib.rs`.

- [ ] **Step 1: failing test** — `ApiClient::new()` builds a base from `resolve_api_base()`; `get_json`/`post_json` return `serde_json::Value`. Test against a `wiremock` mock server: a stubbed `GET /api/health` returns `{"ok":true}`; assert the client parses it.
- [ ] **Step 2:** `cargo test -p agentum-cli api_client_get_health` → FAIL.
- [ ] **Step 3:** implement `ApiClient { base: String, http: reqwest::Client }` with `get_json(path)`, `post_json(path, body)`, `--insecure`/fingerprint reuse from `terminal::api` (extract or duplicate minimally). Loopback http needs no TLS.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(cli): shared ApiClient over resolve_api_base`.

### Task 5: `agentum status --json`

**Files:** Create `commands/status.rs`; modify `cli.rs` (`Cmd::Status { json: bool }`).

- [ ] **Step 1: failing test** — `render_status(sessions, worktrees, hosts)` returns a struct with counts (`sessions_running`, `worktrees`, `hosts_reachable`) and serializes to stable JSON keys. Pure function, table-tested.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** `status::run(json)` calls `GET /api/sessions`, `/api/worktrees`, `/api/hosts`, composes `render_status`, prints JSON or a human table.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(cli): agentum status (composed from server API)`.

### Task 6: `agentum worktree` (list/current/create/rm/set/ps/show)

**Files:** Create `commands/worktree.rs`; modify `cli.rs` (`Cmd::Worktree { action }` with a `WorktreeAction` subcommand enum).

Routes already exist: `GET /api/worktrees`, `POST /api/worktrees/create`, `POST /api/worktrees/remove`, `POST /api/worktrees/update-meta`.

- [ ] **Step 1: failing test** — `worktree_create_body(name, base, repo_id)` builds the exact JSON `/api/worktrees/create` expects (verify shape against `routes/worktrees.rs::create`). `worktree_set_meta_body(id, comment)` for `update-meta`. Table-tested pure builders.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** implement each action as an `ApiClient` call using the builders; `current` resolves from `$AGENTUM_WORKTREE_PATH`/cwd against the worktree list; `comment` maps to `update-meta`.
- [ ] **Step 4:** PASS + manual `agentum worktree list --json`.
- [ ] **Step 5:** commit `feat(cli): agentum worktree subcommands over /api/worktrees`.

### Task 7: `tasks` store table + repo methods

**Files:** Modify `crates/agentum-store/src/lib.rs`; add migration `crates/agentum-store/migrations/NNNN_tasks.sql`.

- [ ] **Step 1: failing test** — `create_task`, `list_tasks(filter)`, `update_task_status`, `add_task_dependency` round-trip in an in-memory store; a task blocked by an incomplete dep reports `ready=false`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** migration: `tasks(id, title, owner, status, created_at)`, `task_deps(task_id, blocked_by)`. Repo methods + a `ready` computed in `list_tasks` (no incomplete blockers).
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(store): tasks + task_deps tables for orchestration`.

### Task 8: `/api/orchestration` routes

**Files:** Create `crates/agentum-server/src/routes/orchestration.rs`; register in `lib.rs::router`.

- [ ] **Step 1: failing test** — axum test: `POST /api/orchestration/tasks` creates; `GET /api/orchestration/tasks` lists; `POST /api/orchestration/tasks/{id}/status` updates. Uses an in-memory store AppState.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** thin handlers over Task 7 methods. Orchestration messaging (`send`/`ask`/`reply`) reuses the existing `/api/channels/*` routes — no new endpoint.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(server): /api/orchestration task DAG routes`.

### Task 9: `agentum orchestration` + `wait` + `exec`

**Files:** Create `commands/orchestration.rs`, `wait.rs`, `exec.rs`; modify `cli.rs`.

- [ ] **Step 1: failing tests** — `orchestration send/ask/reply` map to channel message bodies (builder test); `wait` polls `GET /api/sessions/{id}` pane text for `--text`/`--timeout` (test the predicate `pane_matches(text, needle)` + a timeout loop with an injected clock); `exec` posts to `/api/sessions/{id}/send` and waits for prompt-return (reuse `wait`).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** implement; `wait --url/--selector/--load/--fn` are browser predicates → defer to Phase 3 (return "requires browser; not yet" until then).
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(cli): orchestration, wait, exec subcommands`.

**Phase 2 exit criteria:** inside a desktop pane, `agentum status --json`, `agentum worktree list`, `agentum orchestration task-create --title x`, `agentum orchestration task-list` all work and round-trip through the embedded server. Same commands work against an SSH host (worktree/status route via existing host layer).

---

## Phase 3 — Browser Automation

### Task 10: `DesktopBridge` trait + `serve_embedded_loopback_with_bridge`

**Files:** Modify `crates/agentum-server/src/lib.rs`.

- [ ] **Step 1: failing test** — a fake `DesktopBridge` whose `browser()` returns `{"ok":true}`; assert `AppState` built by `serve_embedded_loopback_with_bridge_state(store, Arc::new(fake))` has `desktop_bridge.is_some()`, and the no-bridge path has `None`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** define `DesktopBridge`, `BrowserOp` (`Tabs`, `Snapshot{tab}`, `Click{tab,selector}`, `Fill{tab,selector,text}`, `Screenshot{tab}`, `Navigate{tab,url}`), `ComputerOp` (stub enum for now), and the `_with_bridge[_state]` constructors.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(server): DesktopBridge trait + bridge-aware embedded boot`.

### Task 11: `/api/browser/*` routes (501 without bridge)

**Files:** Create `crates/agentum-server/src/routes/browser.rs`; register in `router`.

- [ ] **Step 1: failing test** — with `desktop_bridge = None`, `GET /api/browser/tabs` → `501` body `{"error":"requires the agentum desktop app"}`; with the fake bridge, → `200` `{"ok":true}`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** handlers read `state.desktop_bridge`; `None` → 501; `Some` → call `.browser(op).await`, forward JSON.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(server): /api/browser routes gated on DesktopBridge`.

### Task 12: Desktop `impl DesktopBridge` for browser

**Files:** Create `crates/agentum-desktop/src/bridge.rs`; modify `lib.rs:55` to call `_with_bridge`.

- [ ] **Step 1:** (manual integration — no unit test; AppHandle needs a running app) Document the manual check in the task: open two browser tabs, `agentum tab list --json` returns both labels.
- [ ] **Step 2:** N/A (manual).
- [ ] **Step 3:** `struct TauriBridge { app: AppHandle }`; `browser(Snapshot)` → `webview.eval("document.documentElement.outerHTML")` via a oneshot back from an `emit`/`eval` callback; `Click`/`Fill` → `eval` a querySelector script; `Tabs` → enumerate `webview_label`-prefixed webviews; `Screenshot` → Tauri `webview.screenshot` (or `eval` canvas). Reuse `browser_native.rs` helpers (`webview_label`, `get_browser_webview`).
- [ ] **Step 4:** manual check passes.
- [ ] **Step 5:** commit `feat(desktop): TauriBridge drives native webviews for /api/browser`.

### Task 13: `agentum tab/snapshot/click/fill` + browser `wait` predicates

**Files:** Create `commands/browser.rs`; modify `cli.rs`, and `wait.rs` (fill in `--url/--selector/--load/--fn`).

- [ ] **Step 1: failing tests** — body builders for each op (selector/text JSON shape); `wait --selector` polls `/api/browser/snapshot` and tests `snapshot_has_selector(html, sel)`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** implement subcommands over `ApiClient`; wire the deferred browser predicates from Task 9.
- [ ] **Step 4:** PASS + manual drive of a real tab.
- [ ] **Step 5:** commit `feat(cli): browser tab/snapshot/click/fill + wait predicates`.

**Phase 3 exit criteria:** `agentum tab list`, `agentum snapshot --json`, `agentum click --selector ...`, `agentum fill --selector ... --text ...` drive a real browser pane in the running desktop. Against a standalone daemon they return a clear "requires desktop" error.

---

## Phase 4 — macOS Computer-Use AX Engine

> The largest phase. It is independently shippable and gated behind the same `DesktopBridge`. It reuses the `.app` bundle's Accessibility TCC grant fixed on 2026-06-11 — so it MUST run from `/Applications/agentum.app` launched via `open`, never a bare `target/` binary (see memory: the dev-binary launch was why agentum never appeared in the Accessibility list).

### Task 14: AX read engine (`list-apps`, `get-app-state`)

**Files:** Create `crates/agentum-desktop/src/computer/mod.rs`, `ax.rs`.

- [ ] **Step 1:** (manual + small pure tests) Pure test: `flatten_ax_tree` over a synthetic `AxNode` tree produces a flat index list with stable `element-index`. Manual: `agentum computer list-apps --json` includes Finder.
- [ ] **Step 2:** FAIL (pure fn missing).
- [ ] **Step 3:** `ax.rs` wraps `AXUIElementCreateApplication(pid)`, walks `kAXChildrenAttribute`, maps role/title/value/frame into `AxNode`; `flatten_ax_tree` assigns indices; `list_apps()` enumerates running apps via `NSWorkspace`/`CGWindowList`.
- [ ] **Step 4:** pure test PASS; manual list-apps works.
- [ ] **Step 5:** commit `feat(desktop): macOS AX read engine (list-apps, get-app-state)`.

### Task 15: AX action engine (`click`, `set-value`, `type-text`, `press-key`, `scroll`, `hotkey`, `paste-text`)

**Files:** Create `crates/agentum-desktop/src/computer/actions.rs`.

- [ ] **Step 1:** pure tests for input mapping — `key_name_to_keycode("Return") == 36`, `hotkey_parse("CmdOrCtrl+A")` → `(vec![Cmd], A)`. Manual: click an element by index in TextEdit.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** `click` → `AXUIElementPerformAction(kAXPressAction)`; `set-value` → `AXUIElementSetAttributeValue(kAXValueAttribute)`; `type-text`/`press-key`/`hotkey` → `CGEventCreateKeyboardEvent`; `scroll` → `CGEventCreateScrollWheelEvent`; `paste-text` → set pasteboard + Cmd+V.
- [ ] **Step 4:** pure tests PASS; manual actions work in TextEdit.
- [ ] **Step 5:** commit `feat(desktop): macOS AX action engine`.

### Task 16: `ComputerOp` + `/api/computer/*` routes + bridge impl

**Files:** Modify `lib.rs` (`ComputerOp` variants), create `routes/computer.rs`, extend `bridge.rs`.

- [ ] **Step 1: failing test** — no-bridge `GET /api/computer/capabilities` → `501`; fake-bridge → forwards `{"accessibility":true}`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** flesh `ComputerOp` (Capabilities, Permissions, ListApps, GetAppState{app}, Click{app,index}, SetValue{app,index,value}, TypeText{app,text}, PressKey{app,key}, Scroll{...}); routes forward to bridge; `bridge.computer()` dispatches to `computer::` engine; `permissions`/`capabilities` reuse `permissions.rs::macos` probes.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** commit `feat(server+desktop): /api/computer routes over AX engine`.

### Task 17: `agentum computer` subcommands

**Files:** Create `commands/computer.rs`; modify `cli.rs`.

- [ ] **Step 1: failing tests** — body builders for each op match `/api/computer/*` shapes (table-tested).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** implement all `agentum computer …` subcommands (capabilities/permissions/list-apps/get-app-state/click/set-value/type-text/press-key/hotkey/paste-text/scroll) over `ApiClient`, matching the exact flags the `computer-use` skill documents.
- [ ] **Step 4:** PASS + manual end-to-end against Spotify/Finder.
- [ ] **Step 5:** commit `feat(cli): agentum computer subcommands (matches computer-use skill)`.

### Task 18: Make the cards & skills honest about scope

**Files:** Modify `crates/agentum-desktop/ui/src/components/settings/{ComputerUsePane,BrowserUsePane,OrchestrationPane}.tsx`; the three `skills/*/SKILL.md` if any documented flag drifted.

- [ ] **Step 1:** review pass — every command each SKILL.md documents now exists in `agentum --help`. Grep `skills/` command surface vs `cli.rs` arms; list any gap.
- [ ] **Step 2:** N/A.
- [ ] **Step 3:** add a "requires the desktop app; local-machine only" note to the Browser/Computer panes; confirm SSH-host behavior text is accurate.
- [ ] **Step 4:** `grep -rhoE 'agentum [a-z-]+ [a-z-]+' skills/` ⊆ implemented commands.
- [ ] **Step 5:** commit `docs: capability cards/skills match the implemented CLI surface`.

**Phase 4 exit criteria:** from `/Applications/agentum.app`, `agentum computer list-apps --json` and a click/type round-trip work; the three skills' documented commands all resolve; no card advertises a command that doesn't exist.

---

## Self-Review

- **Spec coverage:** Phase 1 (discovery + hook fix) → Tasks 1-3. Phase 2 (status/worktree/orchestration/wait/exec) → Tasks 4-9. Phase 3 (browser) → Tasks 10-13. Phase 4 (computer-use) → Tasks 14-17. One-binary constraint → all CLI work lands in `agentum-cli` (`agentum` binary); no new binary. SSH constraint → Task 6/Task 5 route through existing host layer; browser/computer explicitly local-only (Tasks 11/16 return 501 off-desktop). Honest cards → Task 18. ✓
- **Type consistency:** `DesktopBridge`/`BrowserOp`/`ComputerOp` defined once (Task 10/16), implemented once (Tasks 12/16), consumed by routes (Tasks 11/16). `ApiClient` (Task 4) is the only HTTP path for Tasks 5-9, 13, 17. `resolve_api_base` (Task 3) underlies `ApiClient`. ✓
- **Known risk to validate at execution:** Task 12 browser `eval`-callback plumbing (async result back from a Tauri webview eval) is the least certain interface — spike it first inside Task 12 before building the other browser ops on top.
