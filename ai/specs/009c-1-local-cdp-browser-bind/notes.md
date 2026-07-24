# 009c-1 — Verified technical facts & running notes

> Durable scratch for the local CDP-Chromium + bound-Playwright-MCP slice. Verified
> facts here are load-bearing for the architecture/implementation — don't re-verify blind.

## VERIFIED 2026-06-18 — `@playwright/mcp@latest` CDP binding flags

Ran `npx -y @playwright/mcp@latest --help`. The binding mechanism 009c-1 depends on is
**real and present** in the current package:

- `--cdp-endpoint <endpoint>` — **"CDP endpoint to connect to."** ← the linchpin. Makes the
  MCP **attach** to an already-running CDP browser instead of spawning its own.
- `--cdp-header <headers...>` — CDP headers to send on connect (useful later for the host/
  tunnel case in 009c-2, e.g. auth).
- `--cdp-timeout <ms>` — connect timeout, default **30000ms**.
- `--executable-path <path>` — path to the browser executable (engine selection).
- `--browser <browser>` — browser or chrome channel to use.
- `--isolated` — keep the browser profile in memory (no persistent profile).
- `--extension` — "Connect to a running browser instance" (via the Playwright MCP extension —
  a different attach path than `--cdp-endpoint`).
- `--endpoint <endpoint>` — "Bound browser endpoint to connect to." (distinct from
  `--cdp-endpoint`; note the existence, prefer `--cdp-endpoint` for raw CDP.)
- Existing flags we already use: `--port`, `--host`, `--headless`.

**Implication / concrete mechanism for 009c-1:**
1. agentum launches a Chromium-class browser with `--remote-debugging-port=<N>` (a known
   local port; reuse a dedicated range so it doesn't collide with :8931 or the API port).
2. agentum runs the MCP as `@playwright/mcp@latest --port 8931 --host 127.0.0.1 --cdp-endpoint http://127.0.0.1:<N>`
   — **dropping `--headless`** (we attach, we don't spawn) — so the MCP drives the SAME
   browser the user watches.
3. The agent points at `http://127.0.0.1:8931/mcp` exactly as today; only the MCP's *backing
   browser* changes from hidden-headless to agentum's displayed instance.

This means `playwright_mcp.rs`'s change is additive: a binding variant of the argv that
swaps `--headless` for `--cdp-endpoint <url>`. The agent-facing URL contract is unchanged.

## Current-state grounding (verified by reading, 2026-06-18)

- `playwright_mcp.rs::ensure_playwright_mcp()` → shared `@playwright/mcp` in tmux session
  `agentum-playwright-mcp`, argv `npx -y @playwright/mcp@latest --port 8931 --host 127.0.0.1 --headless`.
  Idempotent on port+session. Returns `http://127.0.0.1:8931/mcp`. Port via `AGENTUM_PLAYWRIGHT_MCP_PORT`.
  Fail-loud pattern: `ensure_npx_available()` bails with install guidance.
- `mcp_provision.rs::provision()` always wires `agentum` McpServer; if `feature_enabled()`
  (`AGENTUM_BROWSER_VERIFY`) wires `playwright` via `ensure_playwright_mcp()`. One combined
  Claude `--mcp-config` (`config_json` → all `type:"http"`). `McpServer{name,url,auth_token}`.
- `browser_native.rs` = Tauri child webview (**WKWebView**), one per page, label prefix
  `browser-page-`. NO CDP. Its own comment: "A true Chromium engine would require the
  host-resident-browser route, not a UA swap." ← the substrate that must change for
  agent-driven/shared tabs.
- `host_browser.rs` does **NOT** exist → **009a unbuilt** → 009c-2 correctly BLOCKED.
- Dormant remote-screencast UI stack exists: `ui/src/shared/browser-screencast-protocol.ts`
  + friends, `RemoteBrowserPagePane` in `BrowserPane.tsx` — disabled ("no runtime backend").
- Tunnel infra: `host_runtime.rs` + `routes/sessions.rs` (forward/reverse). `spawn_agent_into_pane`
  is the shared launch helper; local path threads `agentum_mcp_url` into `provision()`.

## Open questions for the architect (RESOLVED in architecture.md)

1. Chromium engine → **Playwright-managed Chromium**, agentum launches w/ `--remote-debugging-port`.
2. Local live-view → **headed Chromium OS window** (NOT screencast; display exists locally).
3. Shared-context → **one browser, agent+user each own a tab, agentum owns lifecycle**.

---

## Developer progress log

### DONE — `playwright_mcp.rs` bound-mode variant (2026-06-18) ✅ VERIFIED GREEN
- Refactored argv into `build_argv(port, &LaunchMode)` with `LaunchMode::{Headless, BoundToCdp(&str)}`.
- `ensure_playwright_mcp()` → `ensure(LaunchMode::Headless)` (unchanged behavior).
- NEW `ensure_playwright_mcp_bound(cdp_endpoint)` → `ensure(LaunchMode::BoundToCdp(..))`: argv drops
  `--headless`, adds `--cdp-endpoint <endpoint>`. Same agent-facing `…/mcp` URL.
- 3 new unit tests (argv shape) + all existing — **`cargo test -p agentum-server --lib playwright_mcp`
  = 5 passed, 0 failed** (full build 2m24s, then tests instant).
- Documented the headless→bound singleton transition caveat in the doc comment (one stale case;
  not hit on the normal path where the engine choice is fixed at first launch).

### DONE — `cdp_browser.rs` (NEW module) (2026-06-18) ✅ VERIFIED GREEN
- Headed-Chromium launcher, mirrors `playwright_mcp`'s singleton/tmux/idempotent shape. ONE shared
  local CDP browser on fixed port **9300** (env `AGENTUM_CDP_BROWSER_PORT`), tmux `agentum-cdp-browser`,
  idempotent on port+session (no in-memory scope map — singleton, like the MCP server; scopes = 009c-2).
- `ensure_local_cdp_browser() -> Result<String>` (returns CDP endpoint), `stop_local_cdp_browser()`,
  `build_chrome_argv()` (headed, `--remote-debugging-address=127.0.0.1 --remote-debugging-port`,
  isolated `--user-data-dir`, `--no-first-run`), fail-loud `chromium_executable()`.
- `pub mod cdp_browser;` registered in lib.rs.
- **BUG CAUGHT BY LIVE-VERIFY + FIXED**: hardcoded exe path `chrome-mac/Chromium.app/.../Chromium`
  was WRONG — current Playwright (build 1228) ships `chrome-mac-x64/Google Chrome for Testing.app/
  Contents/MacOS/Google Chrome for Testing` (arch-suffixed dir + renamed app). Replaced the hardcoded
  rel-path with **layout-discovery** `find_chrome_exe_in()` (globs `chrome-mac*`/`chrome-linux*`/
  `chrome-win*`, derives the macOS exe from the `*.app` stem — handles both new + legacy names).
  Regression tests `finds_renamed_arch_suffixed_chrome_for_testing` + `finds_legacy_chromium_app_too`.
- Tests: **5 passed** (`cargo test -p agentum-server --lib cdp_browser`).

### DONE — `mcp_provision.rs` engine seam (2026-06-18) ✅ VERIFIED GREEN
- `BrowserMcpEngine{PlaywrightBound, PlaywrightHeadless}` (closed enum — the agnostic seam, NOT a
  plugin framework). `browser_mcp_engine()` → `parse_engine(env AGENTUM_BROWSER_MCP_ENGINE)`; default
  **PlaywrightBound**, `=headless` forces legacy.
- `provision_browser_mcp(engine)`: bound → `cdp_browser::ensure_local_cdp_browser()` →
  `ensure_playwright_mcp_bound(endpoint)`; **degrades to headless on ANY failure** (best-effort, logged
  — a missing local CDP browser never blocks an agent launch). `provision()` calls it where it used to
  call `ensure_playwright_mcp()` directly. `sessions.rs` launch site UNCHANGED (one shape for 009c-2).
- Test `engine_seam_defaults_to_bound_and_honors_headless_override`.

### DONE — full suite + live-verify (2026-06-18) ✅
- **`cargo test -p agentum-server --lib` = 238 passed, 0 failed, 4 ignored** (whole crate clean w/ changes).
- `cargo tauri dev` (running) recompiled `agentum-desktop` against the changed `agentum-server` — builds clean in the app too.
- **LIVE-VERIFY of the core mechanism (headless, non-intrusive, self-cleaning script)**:
  - PASS#1: agentum's exact launch argv → working CDP browser (`Chrome/149.0.7827.55`, Protocol 1.3, webSocketDebuggerUrl).
  - PASS#2: `@playwright/mcp --cdp-endpoint http://127.0.0.1:9388` started + listened + **attached to OUR browser** (did not spawn its own).
  - This proves "Playwright MCP bound to agentum's browser over --cdp-endpoint" works on this machine — the heart of 009c-1.

### Design note — NO per-session teardown (intentional)
The CDP browser is a long-lived shared SINGLETON (one per machine, reused across sessions, survives
agent restarts) — exactly like the Playwright MCP server, which is also never torn down per-session.
`stop_local_cdp_browser()` is for explicit/manual teardown (or the optional stop route), not session end.

### DONE — Reviewer gate (rust-reviewer) + fixes (2026-06-18) ✅ GREEN + clippy clean
Ran the SDD Reviewer (ecc:rust-reviewer) on the 6-file diff → verdict NEEDS-FIX, 5 findings. Fixed the
4 legitimate ones; #5 deferred to 009c-2 (host/Linux):
1. **[HIGH, fixed]** unused `delete` import in `routes/cdp_browser.rs` broke `clippy -D warnings`. Removed.
2. **[HIGH, fixed]** CDP-browser restart left the bound MCP holding a dead CDP WebSocket (next launch reused
   the stale MCP → every tool call fails). Added `playwright_mcp::stop_bound_mcp()`, called from
   `stop_local_cdp_browser()` so teardown resets both.
3. **[MED, fixed]** concurrent `provision()` launches raced on the tmux session → loser got "duplicate
   session" → wrongly degraded to headless. Added a process-wide `tokio::sync::Mutex` launch guard
   (double-checked fast path) in BOTH `cdp_browser::ensure_local_cdp_browser` and `playwright_mcp::ensure`.
4. **[MED, fixed]** `provision_browser_mcp` doc said "degrades on any failure" but only degrades on
   CDP-browser-launch failure — corrected the comment + made the intent explicit (a bound-MCP failure is
   propagated, NOT silently degraded to a hidden browser; that split is exactly what the feature removes).
5. **[LOW, DEFERRED → 009c-2]** no Linux `$DISPLAY`/Wayland pre-check before launching headed Chromium on a
   display-less host → 20s opaque wait then degrade. Irrelevant to 009c-1 (local macOS, display present);
   the host slice (009c-2) on Linux must add this (or use headless+screencast, a different path entirely).
- Verified: `cargo clippy -p agentum-server --lib` clean (exit 0); `cargo test -p agentum-server --lib`
  = **239 passed, 0 failed, 4 ignored**.

009c-1 has now passed PM → Architect → Developer → **Reviewer**. (SDD Tester gate optional — the unit suite
+ live-verify already cover behavior; the one remaining behavior is the human desktop UX acceptance.)

### NEXT — remaining for 009c-1
Steps 1–3 (playwright_mcp bound variant, cdp_browser.rs, mcp_provision seam) are DONE + GREEN, and the
binding mechanism is live-verified headlessly. Remaining:

1. **Manual desktop acceptance proof** (HUMAN — needs the desktop UI + a real agent session): launch the
   desktop, start a local Claude/Codex session with `AGENTUM_BROWSER_VERIFY=1`, watch the headed
   "Google Chrome for Testing" window appear, have the agent call `browser_navigate` → confirm the
   user-visible change in that window. **All underlying mechanism now live-verified** (see below) — this
   is just the end-to-end UX confirmation.
   - ✅ DE-RISKED 2026-06-18: headed Chromium launched **from a tmux detached pane** (the exact
     `cdp_browser::ensure` path, `tmux new-session -d … <exe> --remote-debugging-port …`) comes up,
     serves CDP, is genuinely **headed** (UA `Chrome/149.0.0.0`, NO `HeadlessChrome` token), and stays
     alive/stable under tmux. So the GUI-window-from-tmux concern is closed; no `open -a` workaround needed.
2. **(Optional desktop chip)** a small status chip ("Agent browser running · [stop]") wired to the route
   below. UI-only; deferred — the route gives the desktop everything it needs.
3. **Commit to `staging`** (HUMAN-GATED): stage ONLY the now-**6** changed/new files
   (`playwright_mcp.rs`, `cdp_browser.rs`, `lib.rs`, `mcp_provision.rs`, `routes/cdp_browser.rs`,
   `routes/mod.rs`) — NEVER `git add -A` (foreign WIP in tauri.conf.json + WorktreeCard.tsx +
   vite.config.*). `ai/` specs are gitignored, not committable.

### DONE — fix silent-wrong-browser collision (2026-06-18) ✅ GREEN
- **Bug**: headless + bound Playwright MCP shared ONE tmux session + port (:8931). If a headless server
  from a prior verify run was still listening, `ensure_playwright_mcp_bound` reused it → the agent silently
  drove a HIDDEN browser, breaking the core "drive the browser the user watches" promise. (I'd documented
  this as a caveat; it's a real silent failure — fixed properly.)
- **Fix**: distinct singletons per mode — headless = `agentum-playwright-mcp` / :8931; bound =
  `agentum-playwright-mcp-bound` / :8933 (env `AGENTUM_PLAYWRIGHT_MCP_BOUND_PORT`). `LaunchMode::{tmux_target,
  server_port}` drive `ensure()`. `provision()` wires exactly one engine, so the agent only ever gets the
  correct URL; the other server (if any) lingers unwired. Bound server, once up, is correctly reused (it
  stays attached to the long-lived CDP-browser singleton on :9300).
- Replaced the now-wrong `both_modes_keep_the_same_agent_facing_url` test with
  `headless_and_bound_are_separate_singletons` (asserts distinct targets + distinct default ports).
- Remaining (rarer) edge: if the CDP browser process restarts (crash) while the bound MCP holds a stale
  CDP connection, the MCP may need its `--cdp-timeout`/reconnect — documented, not solved in v1.
- `cargo test -p agentum-server --lib playwright_mcp` = 5 passed.

### DONE — `/api/cdp-browser` status/stop/launch route (2026-06-18) ✅ GREEN
- NEW `routes/cdp_browser.rs`: `GET /api/cdp-browser` (status: running/port/cdp_endpoint, no launch),
  `POST` (ensure/launch — fail-loud install hint surfaces in the body), `DELETE` + `POST /api/cdp-browser/stop`
  (teardown). Authed by default (not in `auth::is_public`). Thin view over `cdp_browser::{is_running,
  ensure_local_cdp_browser, stop_local_cdp_browser}` — never a second launcher. Wires the previously
  unused `stop_local_cdp_browser` into a real surface.
- Added pub helpers `cdp_browser::{port, is_running}`. Registered in `routes/mod.rs` + merged in `lib.rs`.
- This is the in-agentum surface satisfying AC "agentum shows the browser; user can watch it live" —
  agentum can now report/launch/stop the headed browser the agent drives.
- `cargo test -p agentum-server --lib` = **239 passed, 0 failed, 4 ignored**.

### Constraints reminder (shared main checkout)
- NEVER `git add -A`; stage only own hunks. Foreign WIP: tauri.conf.json, WorktreeCard.tsx, vite.config.*.
- `ai/` is gitignored (010a fixes) — spec/arch/notes are on-disk only, not committable yet.
- Release = commit to `staging` (push/release HUMAN-GATED per repo convention).
- `cargo` not on non-interactive PATH → `export PATH="$HOME/.cargo/bin:$PATH"` first.
