# Tasks — 009a (Host Browser: Drive + Watch Live)

> Developer build plan, grounded in `architecture.md` + the two codebase sweeps. Sequenced so
> each phase is independently verifiable. Checkboxes unchecked = not built yet. **Big de-risk:**
> Phase 3 mostly *re-activates* dormant UI infra (see architecture "Key finding").

## Phase 1 — Host browser + forward tunnel (backend, no UI)

- [ ] **T1** `agentum-tmux/src/ssh.rs` — add a `-L` forward-command builder beside `ssh_control_forward_cmd` (`:341`): `ssh -O forward -L 127.0.0.1:<mac>:127.0.0.1:<host>`. Unit-test the emitted argv.
- [ ] **T2** `agentum-server/src/host_runtime.rs` — add `ensure_forward_tunnel(host, host_port) -> Result<u16>` mirroring `ensure_reverse_tunnel` (`:857`): scan a **new** range `REMOTE_CDP_PORT_BASE = 9200` (24 tries) for a free **Mac** loopback port, reuse the Interactive `ControlMaster`, cancel-then-arm. Unit-test port selection.
- [ ] **T3** `agentum-server/src/host_browser.rs` (NEW) — `launch_host_browser(host, workdir)`: build chromium argv (`--headless=new --remote-debugging-address=127.0.0.1 --remote-debugging-port=0 --user-data-dir=/tmp/agentum-hostbrowser-<wt>`), launch via `host_runtime::new_session` (`:646`) into tmux `agentum-hostbrowser-<wt>`; read the bound CDP port from the host-side `DevToolsActivePort` file; write a host marker `~/.agentum/hostbrowser/<wt>.port`. Teardown = `kill_session` (not `C-c`).
- [ ] **Verify P1:** after launch + `ensure_forward_tunnel`, `curl 127.0.0.1:<mac>/json/version` from the Mac returns the host Chromium's CDP banner. (Proves browser-on-host + forward tunnel.)

## Phase 2 — CDP screencast bridge + route (backend)

- [ ] **T4** `host_browser.rs` — CDP client: WS to the page target, `Page.navigate`, `Page.startScreencast{format:'jpeg'}`; re-encode each `Page.screencastFrame` into the **existing** wire format (`ui/src/shared/browser-screencast-protocol.ts`: kind `0x62`, v1, opcode `Frame`, JSON metadata + JPEG); ack frames. Input: map incoming events → `Input.dispatchMouseEvent` / `dispatchKeyEvent`.
- [ ] **T5** `agentum-server/src/routes/host_browser.rs` (NEW) — `POST /api/host-browser` (start-or-attach), `GET /{id}/screencast` (WS: frames out / input in), `POST /{id}/navigate`, `GET /{id}`, `DELETE /{id}`. Register in `routes/mod.rs` + `lib.rs::router` (behind the existing auth layer). TCP-healthcheck the CDP port before returning "ready".
- [ ] **Verify P2:** a scratch WS client receives protocol-framed JPEG frames; a `navigate` + injected click changes subsequent frames. (Pure backend, no desktop yet.)

## Phase 3 — Desktop activation (UI — mostly re-activating dormant infra)

- [ ] **T6** `ui/src/shared/types.ts:551` — add `BrowserPage.renderMode?: 'native' | 'remote-screencast'`; `ui/src/store/slices/browser.ts` — set it on create + reuse `remoteBrowserPageHandlesByPageId`.
- [ ] **T7** `ui/src/components/browser-pane/BrowserPane.tsx:745` — branch to the existing `RemoteBrowserPagePane` (`:764–896`) when `renderMode==='remote-screencast'`; restore its WS subscription + `decodeBrowserScreencastFrame` loop + input listeners (`getRemoteBrowserKeypressKey`, `getRemoteBrowserKeyboardShortcut`); finish `remote-browser-frame-style.ts` (`:4`) using frame metadata. Native path (`NativeBrowserPagePane` / `commands/browser_native.rs`) untouched.
- [ ] **T8** Entry point: from a **host session/worktree**, "Open host browser" creates a browser tab in `remote-screencast` mode bound to `{hostId, workdir}`, wired to `POST /api/host-browser` + the screencast WS.
- [ ] **Verify P3:** in the app, open a host browser → the host app's `localhost:PORT` renders live; click/type works; scoped to the worktree. (AC #1, #2, #3-watch, #4-live.)

## Phase 4 — Continuity, preflight, fail-loud

- [ ] **T9** Preflight: `chromium --version` (or `npx playwright install chromium --dry-run`) on the host; if missing, **offer-install** (`npx playwright install chromium`); if it still can't start, surface a **stated reason** (no silent hang). Reuse the `/api/hosts/{id}/readiness` pattern.
- [ ] **T10** Reconnect: `POST /api/host-browser` is **attach-if-exists** (reads the host marker); reopening agentum re-tunnels + re-subscribes to the same Chromium → live view resumes at current state.
- [ ] **Verify P4 (full AC sweep):** close lid / quit agentum → host browser keeps running → reopen → live view resumes (AC #4). Missing Chromium → offer-install / stated reason (AC #5).

## Acceptance-criteria mapping

| AC (spec 009a) | Tasks |
|---|---|
| Browser launches on host, loads host `localhost:PORT` | T2, T3, T5(navigate) |
| Developer runs tests there, sees results, no terminal | T4–T8 |
| Watch live, scoped to worktree | T4, T7, T8 |
| Survive lid-close/quit; reopen re-attaches | T3(marker), T10 |
| Missing browser → offer-install; else stated reason | T9 |

## Build notes

- **Isolation:** build in a git worktree (feature branch) — the main checkout has unrelated WIP + an in-flight 008a.
- **TDD-able units:** the ssh -L builder (T1), the forward-tunnel port scan (T2), the CDP-frame → protocol re-encoder (T4) are pure functions — test first.
- **Push/release: HUMAN-GATED** (project convention; matches 008a). No commit/push without explicit go.
- **`ai/` is gitignored** — these specs aren't committable; only the `crates/` code would be.
