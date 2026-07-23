
---

## Developer progress log — Rust engine (steps 1-3) DONE + live-verified (2026-06-18)

### DONE — CDP client `cdp_screencast.rs` (step 1) ✅ GREEN
- Added `tokio-tungstenite 0.24` + `futures-util` to `agentum-server/Cargo.toml`
  (mirrors agentum-tui's exact pin → no Cargo.lock feature churn).
- `run_screencast_bridge(cdp_http_base, opts, input_rx, frame_tx)`: discover page-target
  WS via `GET /json` (`pick_page_ws_url`), connect tungstenite, `Page.enable` +
  `Page.startScreencast`; on each `Page.screencastFrame` → `encode_frame` → `frame_tx`
  → **`Page.screencastFrameAck`** (required or the stream stalls after frame 1).
- Input back-channel: `parse_input_message` (pane `browser.*` JSON → `InputCommand`) +
  `input_command_to_cdp` (pure) → `Input.dispatchMouseEvent`/`dispatchKeyEvent`,
  `Page.navigate`, `Page.reload`, `history.back/forward` via `Runtime.evaluate`.
  Mouse down/up reuse the last `mouseMove` position (pane sends coords only on move).
  Printable key = one `char` event; named key (Enter/Backspace/Tab/Esc/Arrows/Delete)
  = keyDown+keyUp w/ VK code (Enter carries "\r").
- 11 unit tests (parse, translate, metadata map, page-ws pick, async frame+ack) — green.

### DONE — WS route `routes/cdp_screencast.rs` (step 2) ✅ GREEN
- `WS /api/cdp-browser/screencast` (axum): splits the pane socket, runs the bridge in a
  task, forwards 0x62 frames as Binary, parses input Text → `InputCommand` (try_send, drops
  on backpressure rather than blocking frames), sends `ready`/`error`/`end` control JSON.
- Query: `cdpPort` (default local `cdp_browser::port()` = 9300; a host browser is the SAME
  `127.0.0.1:<port>` over the 009a ssh -L tunnel → **one bridge, local+host**), `format`,
  `quality`, `maxWidth`, `maxHeight`, `everyNthFrame`. Authed by middleware (`?token=`);
  no-auth on the embedded loopback server so the desktop connects tokenless.
- Registered in `routes/mod.rs` + merged in `lib.rs` router. 1 unit test (query→options).

### DONE — `cdp_browser.rs` → headless (step 3) ✅ GREEN
- `build_chrome_argv`: `--headless=new --hide-scrollbars --window-size=1280,800` (full
  Chromium headless — screencast-capable, NOT the reduced `chromium_headless_shell`).
  Doc comments + the argv test flipped headed→headless. cdp_browser stays a per-machine
  singleton on :9300; `mcp_provision` bound-engine path UNCHANGED (bound MCP still attaches
  to this instance — now the one agentum screencasts).

### LIVE-VERIFY (real headless Chromium) ✅ + BUG CAUGHT
- `tests/cdp_screencast_live.rs` (`#[ignore]`): `ensure_local_cdp_browser` (headless) →
  `run_screencast_bridge` → asserts a valid 0x62 frame arrives, then a `Goto` produces
  another frame (input reaches the same instance). PASS: first frame 5323 img bytes; a
  post-nav frame followed. Self-cleaning (stops the browser).
- **BUG caught by live-verify**: `everyNthFrame:2` (the pane's nominal value) makes Chrome
  drop the *only* frame a static page emits (it sends the 2nd/4th… compositor frame; a
  loaded-but-idle page produces just one) → pane stays BLANK until a repaint. A node `ws`
  probe with `everyNthFrame:1` got the initial frame instantly; the Rust bridge with `:2`
  got nothing. Fix: `ScreencastOptions::default().every_nth_frame = 1` (quality+maxWidth
  bound bandwidth anyway). The rewired client (step 4) must request `everyNthFrame:1`.

### Test counts
- `cargo test -p agentum-server --lib` = **252 passed / 0 failed / 4 ignored** (was 243; +9
  cdp_screencast unit + route test). `cargo clippy -p agentum-server --lib` clean; the new
  integration test clippy-clean.
- PRE-EXISTING (not mine, not regressed): `tests/goal_cards_end_to_end.rs` +
  `tests/card_session_binding_e2e.rs` fail to compile — construct `AppState` without
  `mcp_token` (field added in b872410, before this work). Untouched; out of 009c-3 scope.

### NEXT — step 4 (client rewire, NEEDS the npm/Vite UI build to verify)
Point `RemoteBrowserPagePane` at `WS /api/cdp-browser/screencast` on the embedded server
(not the stubbed native `runtime_environments_subscribe`); flip pane selection for agent
pages; reuse the 0x62 decoder + `<img>` render + `remote-browser-keyboard.ts` serializer;
request `everyNthFrame:1`; declare `browser.screencast.v1` in `shared/protocol-version.ts`;
replace the `AGENTUM_BROWSER_VERIFY` env gate with a Settings toggle.

---

## Developer progress log — step 4 (client rewire) DONE + bundle-verified (2026-06-18)

Chose a CONTAINED activation over a blind surgical rewrite of the 5,010-line legacy
`RemoteBrowserPagePane` (tightly coupled to the stubbed `runtimeEnvironments` model:
status.get / tabs / viewport-RPC / dialogs the single-page screencast WS has no concept
of). A blind half-rewire would bundle green but risk runtime breakage I can't verify
headlessly — exactly what the shared-checkout constraints warn against. Instead:

- **Server capability** (`routes/health.rs`): `/api/health` now advertises
  `browser.screencast.v1` so the client can feature-detect the bridge. (cargo-verified.)
- **NEW transport client** `ui/src/runtime/cdp-screencast-client.ts` — `openCdpScreencast`:
  builds `wsUrl('/api/cdp-browser/screencast')` + `?token=` + screencast query, opens the
  WS (binaryType=arraybuffer), decodes nothing itself (hands raw bytes to `onBinary` so the
  pane reuses the FIXED `decodeBrowserScreencastFrame`), parses `ready/error/end` control
  JSON, `sendInput(method, params)` → `{method,params}` JSON, capped-backoff reconnect,
  `close()`. Callback names (`onBinary/onError/onClose`) mirror the legacy subscribe so the
  surface is familiar. **9 vitest tests** (url/token/query, frame passthrough+ready,
  input serialize, drop-while-closed, error/end control, reconnect, give-up, close) — GREEN.
- **NEW pane** `ui/src/components/browser-pane/AgentBrowserScreencastPane.tsx` — `<img>` from
  decoded-frame ObjectURL (revokes prev), mouse move/down/up/wheel with the contract's
  device-coordinate normalization (`round(((clientX-rectLeft)/rectWidth)*deviceWidth)`),
  keyboard via the existing `remote-browser-keyboard.ts` serializer (Space→' '; Meta/Ctrl+r
  → reload), address bar goto + back/forward/reload. Opens the screencast only while the pane
  is the active surface; tears down on background/unmount (no parked socket).
- **Pane selection** (`BrowserPane.tsx`): renders `AgentBrowserScreencastPane` when
  `settings.agentBrowserScreencast` is ON **and** the page has a remote handle
  (`remoteBrowserPageHandlesByPageId[pageId]`) — else the native WKWebView pane, unchanged.
  Default-off + handle-gated → zero regression for ordinary browsing.
- **Settings toggle replaces the env gate**: `agentBrowserScreencast: boolean` added to
  `GlobalSettings` (types.ts) + default `false` (constants.ts) + an "Agent browser in pane"
  switch in `ExperimentalPane.tsx` (+ search registry entry). No more `AGENTUM_BROWSER_VERIFY`
  hand-set for the rendering surface.
- **Keyboard completeness**: extended the server `named_key` map with Home/End/PageUp/PageDown
  VK codes (the serializer emits them) so they dispatch as key events, not literal text.

VERIFICATION (what's possible headlessly): `npm run build` (vite) bundles clean with all the
client changes; the new transport client has full vitest coverage; the Rust engine + capability
+ named_key all cargo-test green (252 lib) + clippy clean; the live integration test still passes
end-to-end against a fresh headless browser.

REMAINING (human desktop acceptance — same category as 009c-1's): launch `cargo tauri dev`,
enable the "Agent browser in pane" setting, mark a page agent-driven, and confirm the live
in-app loop (agent `browser_navigate` → pane repaints; user click/type/scroll reaches the page).
FOLLOW-UPS (out of this slice): auto-marking a page agent-driven when the agent opens a CDP page
(no UI signal exists yet — currently the remote-handle store is the gate); host `cdpPort` plumbing
through the handle for the 009a-tunnel case; tab/dialog parity with the legacy pane.

### Files touched in step 4 (all mine; NONE are foreign WIP)
- crates/agentum-server/src/routes/health.rs            (capability)
- crates/agentum-server/src/cdp_screencast.rs           (named_key Home/End/PageUp/PageDown)
- crates/agentum-desktop/ui/src/runtime/cdp-screencast-client.ts            (NEW)
- crates/agentum-desktop/ui/src/runtime/cdp-screencast-client.test.ts       (NEW)
- crates/agentum-desktop/ui/src/components/browser-pane/AgentBrowserScreencastPane.tsx (NEW)
- crates/agentum-desktop/ui/src/components/browser-pane/BrowserPane.tsx     (import + selection)
- crates/agentum-desktop/ui/src/components/settings/ExperimentalPane.tsx    (toggle)
- crates/agentum-desktop/ui/src/components/settings/experimental-search.ts  (search entry)
- crates/agentum-desktop/ui/src/shared/types.ts                            (setting field)
- crates/agentum-desktop/ui/src/shared/constants.ts                        (default false)
