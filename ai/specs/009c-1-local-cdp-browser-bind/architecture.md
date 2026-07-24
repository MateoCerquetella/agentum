# Architecture Notes — 009c-1 (Local CDP-Chromium Browser + Bound Playwright MCP)

> Grounded first-hand this session by reading: `playwright_mcp.rs`, `mcp_provision.rs`, `routes/sessions.rs::spawn_agent_into_pane`, `browser_native.rs`, the dormant `RemoteBrowserPagePane` + `browser-screencast-protocol.ts`, and `009a/architecture.md`. The `@playwright/mcp@latest --cdp-endpoint` flag is **verified present** (`notes.md`, 2026-06-18). This is the **local de-risking slice** of 009c — it proves the "one browser" model on the easy substrate. 009c-2 reuses this exact wiring shape on an SSH host.

---

## The three load-bearing decisions

### Decision 1 — Engine: **Playwright-managed Chromium**, launched by agentum with `--remote-debugging-port`

**Chosen:** the Chromium binary Playwright already manages (`npx playwright install chromium` → a known cache path under `~/Library/Caches/ms-playwright/chromium-*/...`). agentum launches it itself with `--remote-debugging-address=127.0.0.1 --remote-debugging-port=<N> --user-data-dir=<isolated>`, then points the MCP at that port via `--cdp-endpoint`.

**Rationale:**
- **Zero new dependency.** `AGENTUM_BROWSER_VERIFY` already requires `@playwright/mcp`, whose install pulls the same Chromium. The fail-loud message in `playwright_mcp.rs` already says "run `npx playwright install chromium`." We reuse that exact prerequisite — no new toolchain, no separate detection surface.
- **Known binary path.** Playwright exposes its resolved executable (`npx playwright install --dry-run` prints the path; or `node -e "console.log(require('playwright').chromium.executablePath())"`). agentum resolves it once and launches it — no PATH guessing, no "is it Chrome or Chromium" ambiguity.
- **Same engine the MCP expects.** Binding an `@playwright/mcp` to a Chromium it didn't manage works over raw CDP, but using *Playwright's own* Chromium guarantees protocol-version compatibility.

**Alternatives rejected:**
- *System Chrome / `--browser chrome` channel* — relies on the user having Chrome installed at a discoverable path; version drift vs Playwright; muddies the fail-loud story (two install paths). Rejected for surface.
- *ungoogled-chromium* (the earlier user ask) — a third install source agentum would have to detect, download, and version-manage. Pure new surface for no 009c-1 value. Rejected as YAGNI; can be swapped in later behind the engine seam (Decision 6) via `--executable-path` with **zero** wiring change.
- *Reuse the `:8931` headless Chromium the MCP already spawns* — defeats the whole spec: that instance is hidden and the user can't watch it. Rejected by the goal.

---

### Decision 2 — Local live-view: **headed Chromium as a real OS window** (NOT screencast)

**Chosen:** for 009c-1 (local), launch Chromium **headed** (drop `--headless`). The user watches the **actual Chromium window** the OS shows. agentum does **not** embed it and does **not** screencast it for the local case.

**Rationale:**
- **A WKWebView cannot render another browser's content** — that's the hard constraint. The only ways to show a CDP Chromium *inside* agentum are screencast (heavy) or OS-level embedding (fragile, not supported by Tauri). 009a already chose CDP screencast for the **remote** case *because the host has no display*. **Locally the display exists** — the simplest correct answer is to let Chromium draw its own window.
- **The spec explicitly flags live-view as "the heaviest UI slice" and says "keep it minimal for 009c-1."** A real OS window is the minimal live-view that still satisfies "the user watches it live" and "an agent action produces a user-visible change in the watched tab." The unification proof is the same: one instance, one window, agent + user both act on it.
- **De-risks the core question (shared context) without paying the screencast tax.** We prove ownership/no-race semantics first; the screencast embed is additive later.
- **Inherits 009a's decision where one exists, decisively differs where the substrate differs.** 009a: screencast *because no display on the host*. 009c-1: OS window *because there is a display locally*. Same engine + same CDP, different presentation justified by substrate. This is not re-litigation — it's the documented local specialization, and the wiring (engine + `--cdp-endpoint`) is identical, so 009c-2 still reuses the shape.

**Alternatives rejected:**
- *CDP screencast into the dormant `RemoteBrowserPagePane`* — this is 009a's path and **will be reused by 009c-2**. For 009c-1 it is over-engineering: it requires the server-side CDP→frame bridge + activating the UI before we've proven the shared-context model. Explicitly deferred. (Noted as the **upgrade path**: once 009a ships its `host_browser.rs` CDP bridge, a `renderMode: 'local-screencast'` could later embed the local Chromium too — but that is NOT 009c-1.)
- *Embed Chromium in a Tauri window via OS tricks* — unsupported, fragile, platform-specific. Rejected.

**Consequence to accept:** locally the agent-driven browser is a separate OS window, not pixels-inside-agentum. That is the honest minimal slice. agentum still owns its lifecycle (launch/focus/teardown) and the address/status surface in-app.

---

### Decision 3 — Shared one-CDP-context lifecycle: **one shared browser, agent and user each own a tab; agentum owns lifecycle**

This is the core risk. The model:

- **One Chromium instance, one default browser context**, launched and owned by agentum (server-side: it holds the `--remote-debugging-port`, the user-data-dir, and the teardown).
- **The agent gets a dedicated tab/page in that shared browser.** When the bound Playwright MCP attaches over `--cdp-endpoint`, it attaches to the **browser**, and its `browser_navigate`/`click`/`snapshot` act on **its page**. The user watches that same window and *can* see the agent's tab; the user may also open their own tabs in the same window.
- **No shared mutable focus race on a single tab.** The "one instance, not two" proof is satisfied at the **browser** level (same window the user watches), not by forcing user and agent to fight over one tab's navigation. Playwright MCP's snapshot/click target the page it controls; the user driving a *different* tab never collides.
- **agentum is the single lifecycle owner.** Launch (agentum spawns Chromium), teardown (agentum kills the process + cleans the isolated user-data-dir on session/browser close), reconnect (agentum re-attaches the MCP to the still-running browser by its known port). Neither the user nor the agent tears down the browser; closing the agentum browser surface kills the OS window.
- **Navigation ownership rule:** the agent owns its tab's navigation; the user owns the user's tabs. The *initial* URL (the page the user wants to share with the agent) is set by agentum at launch via CDP `Page.navigate` on the agent's page, so "drive my browser" starts on the page the user pointed at.

**Rationale:** a single-shared-tab "both steer the same page" model is the race-prone trap the spec warns against (focus/navigation/teardown contention). Per-tab ownership in one shared browser is the **no-race** model: it preserves the user-visible unification ("the window I'm watching is the one the agent drives") while giving each actor an isolated surface. It maps cleanly onto CDP (one browser endpoint, N pages) and onto Playwright MCP (it drives a page).

**Alternatives rejected:**
- *One shared tab, both drive it* — race on navigation/focus; spec's named failure mode. Rejected.
- *Agent gets its own context (incognito-style)* — would visually isolate the agent from the user (different window region/profile), breaking "the same window I'm watching." Rejected; we want one window, shared context, separate tabs.

---

## Component / seam map (file → change)

| File | Change |
|---|---|
| **NEW** `crates/agentum-server/src/cdp_browser.rs` | Local CDP-Chromium **launcher + lifecycle owner**. (a) resolve the Playwright Chromium executable path (fail-loud if absent); (b) pick a port in a dedicated local range (`LOCAL_CDP_PORT_BASE = 9300`, scan upward — distinct from 009a's host range 9200+ and the MCP `:8931`); (c) launch headed Chromium in a tmux session (`agentum-cdp-browser-<scope>`) with `--remote-debugging-address=127.0.0.1 --remote-debugging-port=<N> --user-data-dir=<isolated> --no-first-run --no-default-browser-check`; (d) read back the bound port from the `DevToolsActivePort` file under the user-data-dir; (e) TCP-healthcheck `http://127.0.0.1:<N>/json/version` before "ready"; (f) hold a lifecycle map `{scope → (tmux_target, cdp_port)}` for reconnect/idempotency; (g) teardown via `kill_session` + remove the isolated user-data-dir. Mirrors `playwright_mcp.rs`'s idempotent-tmux-singleton shape and 009a's `host_browser.rs` shape so 009c-2 generalizes it. |
| `crates/agentum-server/src/playwright_mcp.rs` | Add a **binding-mode argv variant**. Keep `ensure_playwright_mcp()` (headless, untouched default). Add `ensure_playwright_mcp_bound(cdp_endpoint: &str) -> Result<String>`: same as today but the argv **drops `--headless` and appends `--cdp-endpoint <cdp_endpoint>`** (per `notes.md` — verified flag). It returns the **same** `http://127.0.0.1:<port>/mcp` agent-facing URL — only the MCP's backing browser changes. Reuse `ensure_npx_available()` fail-loud. **Open detail:** the headless and bound MCP must not collide on `:8931`; either reuse the singleton (bound replaces headless when binding is requested) or give the bound MCP its own port. Recommend: the bound MCP **is** the singleton when `AGENTUM_BROWSER_VERIFY` + a CDP browser exist — there's only ever one Playwright MCP per machine, and 009c-1 makes it the bound one for agent-driven tabs. |
| `crates/agentum-server/src/mcp_provision.rs` | In `provision()`, where it currently calls `ensure_playwright_mcp()`: **branch through the engine seam** (Decision 6). When a local CDP browser is available/launchable, call `cdp_browser::ensure(...)` → get `cdp_endpoint`, then `ensure_playwright_mcp_bound(cdp_endpoint)`; push the `playwright` `McpServer` with the same URL contract. The `McpServer { name, url, auth_token }` shape is **unchanged**. Fall back to today's headless path on any failure (best-effort, logged) so a missing CDP browser degrades, never hangs. |
| `crates/agentum-server/src/routes/sessions.rs` (`spawn_agent_into_pane`, local branch) | **No structural change** — it already threads `agentum_mcp_url` into `provision()` and extends argv with `adapter.mcp_args(&p)`. The CDP launch + binding happens *inside* `provision()` → `cdp_browser` + `playwright_mcp`. This keeps the launch site dumb and the wiring shape identical for 009c-2's remote branch. |
| `crates/agentum-server/src/lib.rs` | `pub mod cdp_browser;` |
| `crates/agentum-desktop/src/commands/browser_native.rs` | **UNCHANGED.** WKWebView lightweight path stays exactly as is (Boundaries below). The agent-driven CDP browser is a *separate* OS window owned by the server, not a Tauri child webview. |
| Desktop UI | **Minimal/none for the OS-window live-view.** Optionally a small status surface (a chip/row showing "Agent browser running · <url> · [stop]") that calls a new status/stop route, but the *content* is the OS Chromium window — no `RemoteBrowserPagePane` activation in 009c-1. The dormant screencast UI stays dormant (it's 009a/009c-2's). |
| **NEW (small, optional)** `crates/agentum-server/src/routes/cdp_browser.rs` — status/stop route | `GET /api/cdp-browser` (running? port? url?) + `DELETE /api/cdp-browser/{scope}` (teardown). Only if the desktop wants a stop button. Defer if the session-teardown hook is enough. |

---

## Data / control flow — "agent action → user sees it in the shared browser"

1. User starts a local agent session with `AGENTUM_BROWSER_VERIFY` on. `spawn_agent_into_pane` (local branch) calls `mcp_provision::provision(state, tool, agentum_mcp_url)`.
2. `provision()` → engine seam → `cdp_browser::ensure(scope)`:
   - resolve Playwright Chromium path (**fail-loud + offer-install** if missing — Decision 5);
   - launch headed Chromium in tmux with `--remote-debugging-port=<N> --user-data-dir=<iso>`;
   - read `DevToolsActivePort`, healthcheck `/json/version`, return `cdp_endpoint = http://127.0.0.1:<N>`.
3. `provision()` → `playwright_mcp::ensure_playwright_mcp_bound(cdp_endpoint)`: starts (or reuses) the MCP with `--cdp-endpoint <cdp_endpoint>` (no `--headless`). The MCP **attaches** to the already-running Chromium. Returns `http://127.0.0.1:8931/mcp`.
4. `provision()` writes the combined `--mcp-config` (agentum + playwright, both `type:"http"`) — **unchanged JSON shape**. The agent argv gets `--mcp-config <file>` (Claude) / `-c` blocks (Codex).
5. agentum CDP-navigates the agent's page to the user's chosen start URL (`Page.navigate`). The **headed Chromium OS window shows it**; the user watches.
6. Agent calls `browser_navigate`/`click`/`snapshot` → the bound Playwright MCP drives **its page in that same Chromium** → the OS window repaints → **the user sees the change live** in the window they're watching. One instance, not two. ✓ (acceptance proof)
7. Session ends / user stops the browser → `cdp_browser` `kill_session` + clean user-data-dir; the bound MCP can stay (singleton) or be reset.

**Reconnect:** reopening agentum / a new session finds the still-running tmux Chromium by deterministic name + known port and re-attaches the MCP — no relaunch (mirrors 009a's reconnect; sets up 009c-2).

---

## Dependency detection + fail-loud (mirrors `ensure_npx_available`)

`cdp_browser::ensure` runs a preflight before launch, in this order, each **fail-loud with a stated reason**:

1. **`npx` on PATH** — reuse the existing `binary_on_path("npx")` check; same message ("Install Node, then `npx playwright install chromium`").
2. **Playwright Chromium present** — resolve the executable path; if it doesn't exist on disk, return a descriptive error: *"Chromium for the agent browser isn't installed. Run `npx playwright install chromium`."* This is the **offer-to-install** surface (the desktop can show a button that runs that command; the server returns the actionable message either way).
3. **CDP port came up** — TCP-healthcheck `/json/version` within a deadline (reuse `wait_until_listening` shape). If it never binds, error: *"Chromium launched but did not expose CDP on 127.0.0.1:<N> within <T>s — check the tmux pane `agentum-cdp-browser-<scope>`."*

Never a silent hang: every branch returns `Result` with a stated reason; `provision()` logs + degrades to the headless `:8931` path (or no Playwright) rather than blocking the launch — exactly today's best-effort contract.

---

## The MCP-agnostic seam (config/registration, NOT a plugin framework)

Add a tiny enum + descriptor in `mcp_provision.rs`:

```
enum BrowserMcpEngine { PlaywrightBound, PlaywrightHeadless }   // concrete, closed set
```

- `provision()` selects the engine from a single function (`browser_mcp_engine()`), defaulting to `PlaywrightBound` when a local CDP browser is launchable, else `PlaywrightHeadless` (today's behavior). The selection reads config/env — **not** a registry of arbitrary plugins.
- The seam is the **branch point + the `McpServer` contract** (`{name, url, auth_token}`), which is already engine-neutral. A future engine adds an arm to the enum and a launcher fn — no plugin loader, no dynamic dispatch, no config schema for third-party MCPs.
- This satisfies "behind a seam, verified by that seam, not a plugin framework" (acceptance criterion) and "Playwright is the concrete reference binding, not hardcoded as the only option."

The engine selection is the same enum 009c-2 will reuse for the host (it just sources the `cdp_endpoint` from a tunnel instead of a local port) — one wiring shape, as the spec demands.

---

## What stays UNCHANGED (explicit boundaries)

- **WKWebView + the `agentum_browser` lightweight path** — `browser_native.rs`, `NativeBrowserPagePane`, the `/api/browser/*` DesktopBridge ops (open/navigate/grab/annotate over WKWebView) are **untouched**. Casual browsing is unaffected. The CDP browser is a **sibling** path scoped to agent-driven/shared tabs only.
- **`ensure_playwright_mcp()` (headless default)** — kept verbatim as the fallback; only an *additive* bound variant is introduced.
- **`spawn_agent_into_pane` structure**, the combined `--mcp-config` JSON shape, `McpServer`/`McpProvision`, YOLO translation, pane_env, the Claude `--settings` hook — all unchanged.
- **The dormant remote-screencast UI** (`RemoteBrowserPagePane`, `browser-screencast-protocol.ts`) — stays dormant; it belongs to 009a/009c-2. 009c-1 does not activate it.
- **The remote SSH branch** of `spawn_agent_into_pane` — untouched (that's 009c-2, gated on 009a).

---

## Risks + mitigations

| Risk | Mitigation |
|---|---|
| **Bound vs headless MCP collide on `:8931`** | Treat the Playwright MCP as a single per-machine singleton; the bound variant *replaces* headless when a CDP browser exists. Idempotent on port+session, exactly like today. Don't run both. |
| **CDP port-bind race** (Chromium not yet listening) | Read `DevToolsActivePort`; TCP-healthcheck `/json/version` before returning "ready" (reuse `wait_until_listening`). |
| **Shared-context contention** | Per-tab ownership (Decision 3): agent drives its page, user drives theirs; one browser, no shared-tab fight. agentum is the sole lifecycle owner. |
| **Live-view is "just an OS window," not embedded** | Accepted, documented minimal slice. Screencast embed is the explicit deferred upgrade (009a backend reuse) — not 009c-1 scope. |
| **Headed Chromium steals focus / lingers** | `--no-first-run --no-default-browser-check`, isolated `--user-data-dir`, deterministic tmux name, `kill_session` teardown + dir cleanup. |
| **Missing Chromium/Playwright** | Three-stage fail-loud preflight with the existing install message; best-effort degrade to headless, never hang. |
| **Local path drifts from the host path (009c-2)** | One engine enum + one `cdp_endpoint`-string contract threaded through `provision()`; `cdp_browser.rs` mirrors `host_browser.rs`'s shape. Resist a local-only special case in `sessions.rs` — keep launch dumb. |
| **Security** | CDP binds `127.0.0.1` only (`--remote-debugging-address=127.0.0.1`); never a public interface. (009c-2 reaches it only via the authenticated SSH tunnel.) |

---

## YAGNI

One new module (`cdp_browser.rs`), one additive MCP argv variant, one engine enum (closed set), one dedicated local port range, OS-window live-view (no screencast, no embed). No ungoogled-chromium download manager, no plugin framework, no screencast bridge, no WKWebView removal. Everything heavier is a later spec (009c-2 / future embed).

---

## Files to touch (for the Developer)

- **NEW** `crates/agentum-server/src/cdp_browser.rs` — launcher + lifecycle + preflight + reconnect map
- `crates/agentum-server/src/playwright_mcp.rs` — add `ensure_playwright_mcp_bound(cdp_endpoint)` (drop `--headless`, add `--cdp-endpoint`)
- `crates/agentum-server/src/mcp_provision.rs` — engine seam (`BrowserMcpEngine`) + branch to bound path in `provision()`
- `crates/agentum-server/src/lib.rs` — `pub mod cdp_browser;`
- **OPTIONAL NEW** `crates/agentum-server/src/routes/cdp_browser.rs` (+ register) — status/stop route, only if the desktop wants a stop button
- **OPTIONAL** small desktop UI status chip (no `RemoteBrowserPagePane` activation)
- Tests: `playwright_mcp.rs` bound-argv unit test (asserts `--cdp-endpoint` present, `--headless` absent, agent URL unchanged); `mcp_provision.rs` engine-seam selection test; `cdp_browser.rs` preflight fail-loud tests

---

## Summary of key decisions + recommended build order

**Key decisions:**
1. **Engine** = Playwright-managed Chromium, launched by agentum with `--remote-debugging-port` (zero new dependency, known path, swappable via `--executable-path` behind the seam).
2. **Local live-view** = headed Chromium as a real OS window (NOT screencast). Minimal correct slice; locally a display exists, so don't pay 009a's screencast tax. Screencast embed is the documented deferred upgrade.
3. **Shared-context model** = one shared browser, agent and user each own a tab, agentum owns lifecycle. No single-tab race.
4. **`--cdp-endpoint` wiring** = additive `ensure_playwright_mcp_bound()` argv variant; the CDP endpoint originates from the new `cdp_browser.rs` (which owns the port); `provision()` threads it; the launch site (`sessions.rs`) is unchanged. Same shape 009c-2 reuses with a tunneled endpoint.
5. **Fail-loud** = three-stage preflight (npx → Chromium-on-disk → CDP-port-up), each with a stated reason + offer-install, degrade to headless, never hang.
6. **Agnostic seam** = a closed `BrowserMcpEngine` enum + the existing engine-neutral `McpServer` contract. Not a plugin framework.

**Recommended build order:**
1. `cdp_browser.rs` — launch headed Chromium with `--remote-debugging-port`, read `DevToolsActivePort`, healthcheck, lifecycle map, three-stage fail-loud preflight. (Standalone-testable; the riskiest new piece.)
2. `playwright_mcp.rs` — `ensure_playwright_mcp_bound(cdp_endpoint)` + unit test (asserts `--cdp-endpoint` in, `--headless` out, agent URL unchanged).
3. `mcp_provision.rs` — `BrowserMcpEngine` seam + branch in `provision()` to launch CDP browser → bind MCP, with best-effort degrade-to-headless. Seam-selection test.
4. **End-to-end live-verify:** local Claude/Codex session with `AGENTUM_BROWSER_VERIFY=1` → an agent `browser_navigate` produces a visible change in the headed Chromium OS window. This is the acceptance proof (one instance, not two).
5. (Optional) status/stop route + desktop chip.

Steps 1–3 are server-only Rust (testable with `cargo test -p agentum-server`); step 4 is the manual live proof; step 5 is optional polish.

**Open question flagged for Developer/PM:** confirm the singleton-replacement strategy for the Playwright MCP (bound *replaces* headless on `:8931`) is acceptable vs giving the bound MCP a separate port — recommend replacement (one MCP per machine), but it touches the `feature_enabled()`/idempotency assumptions in `playwright_mcp.rs`.
