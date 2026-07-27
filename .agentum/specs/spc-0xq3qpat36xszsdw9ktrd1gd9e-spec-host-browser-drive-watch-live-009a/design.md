# Architecture Notes — 009a (Host Browser: Drive + Watch Live)

> Grounded first-hand in `33b748b` + two codebase sweeps (host_runtime/tunneling; desktop
> browser-pane). **Key finding: the desktop already has dormant remote-screencast infra**
> (frame protocol, input serialization, a `RemoteBrowserPagePane`, store handles) — disabled
> because "no runtime backend in this port." 009a **supplies that backend** and re-activates
> the UI, rather than building a renderer from scratch. That materially de-risks the live-view.

## Components

**Browser runs ON the host; only JPEG frames + input events cross the SSH tunnel.**

1. **Host-side persistent browser (NEW launch path)** — `chromium --headless=new --remote-debugging-address=127.0.0.1 --remote-debugging-port=<P> --user-data-dir=/tmp/agentum-hostbrowser-<wt>` in its **own persistent tmux session** via `host_runtime::new_session(host, target, workdir, cmd, env)` (`host_runtime.rs:646`). tmux name deterministic per worktree (`agentum_tmux::target_for` → `agentum-hostbrowser-<wt>`), so it survives Mac sleep / agentum close and reconnect is a lookup. Teardown via `kill_session` (`host_runtime.rs:820` pattern) — **not** graceful `C-c` (headless Chromium ignores it).
2. **Forward tunnel (NEW)** — `host_runtime::ensure_forward_tunnel(host, host_cdp_port) -> mac_port`, mirroring `ensure_reverse_tunnel` (`host_runtime.rs:857`) but emitting `ssh -O forward -L 127.0.0.1:<mac>:127.0.0.1:<host_cdp>` (new sibling of `ssh_control_forward_cmd`, `agentum-tmux/src/ssh.rs:341`; cancel via `ssh_control_cancel_cmd:374`). Reuses the **Interactive** ControlMaster (`control_path_for(SshMux::Interactive)`). **Separate port range** `REMOTE_CDP_PORT_BASE = 9200` (24 tries) so it never collides with the MCP reverse range (8990+). *Direction matters: CDP lives on the host → the Mac needs a **forward** (-L) tunnel, the mirror of the reverse (-R) MCP tunnel.*
3. **Server CDP→screencast bridge (NEW)** — `crates/agentum-server/src/host_browser.rs`: TCP-healthcheck the CDP port through the tunnel, connect a CDP client at `127.0.0.1:<mac_port>`, `Page.navigate` to the host app URL, `Page.startScreencast{format:jpeg}`. Re-encode each CDP frame into the **existing** wire format (`ui/src/shared/browser-screencast-protocol.ts` — kind `0x62`, v1, opcode `Frame`) and push over a WS route; accept input events and dispatch via CDP `Input.dispatch{Mouse,Key}Event`. Owns the lifecycle map `{ worktree → (tmux_target, host_cdp_port, mac_port) }`.
4. **Desktop live-view (ACTIVATE dormant infra — minimal new code)** — add `renderMode: 'native' | 'remote-screencast'` to `BrowserPage` (`ui/src/shared/types.ts:551`), branch in `BrowserPane.tsx:745` to the **existing** `RemoteBrowserPagePane` (`BrowserPane.tsx:764–896`); restore its WS subscription + frame-decode loop (`decodeBrowserScreencastFrame`) + input listeners (`getRemoteBrowserKeypressKey` / `getRemoteBrowserKeyboardShortcut`); finish the `remote-browser-frame-style.ts` stub. The **local native webview** (`src/commands/browser_native.rs` — real OS child webview) is untouched.

---

## APIs

New route file `crates/agentum-server/src/routes/host_browser.rs` (register in `routes/mod.rs` + `lib.rs::router`):

| Method/Path | Purpose |
|---|---|
| `POST /api/host-browser` | Start **or re-attach** for `{host_id, workdir}`: launch (or find) the tmux browser, forward-tunnel its CDP, return `{id, attached}` |
| `GET /api/host-browser/{id}/screencast` (WS) | Frames **out** (existing `browser-screencast-protocol` encoding); input **in** (mouse/key/scroll) |
| `POST /api/host-browser/{id}/navigate` | `{url}` → CDP `Page.navigate` (load the host app's `localhost:PORT`) |
| `GET /api/host-browser/{id}` | Status: running / current URL / tunnel up / preflight result |
| `DELETE /api/host-browser/{id}` | `kill_session` + drop the forward tunnel |

The WS frame shape **conforms to the existing UI protocol** so the dormant `RemoteBrowserPagePane` consumes it directly.

---

## Data Flow

1. In a **host session**, the user opens a host browser for the worktree → `POST /api/host-browser`.
2. Server: `host_runtime::new_session` launches headless Chromium with `--remote-debugging-port=<P>` in a persistent tmux session on the host; reads back the bound port (DevToolsActivePort file under `--user-data-dir`); records `{worktree → port}` in a host-side marker so reconnect can find it.
3. `ensure_forward_tunnel` → `ssh -L 127.0.0.1:<mac> → host:127.0.0.1:<P>` over the warm Interactive ControlMaster; **TCP-healthcheck** before "ready".
4. CDP client at `127.0.0.1:<mac>` → `Page.navigate` (host app URL) → `Page.startScreencast`. Each frame re-encoded to the existing protocol and pushed on the screencast WS.
5. Desktop `RemoteBrowserPagePane` decodes frames → `<img>`; user input → WS → CDP `Input.dispatch*`.
6. **Mac sleeps / agentum quits** → tmux Chromium keeps running on the host; the -L tunnel drops with the SSH channel.
7. **Reopen agentum** → server reads the host marker, re-establishes the -L tunnel, re-attaches CDP screencast to the **same** Chromium → live view resumes at current state ("see the progress on return").

---

## Important Decisions

- **Re-activate the dormant remote-screencast UI, don't rebuild** — the frame protocol, input serialization, store handles, and `RemoteBrowserPagePane` already exist (disabled). _Chose reuse because the port deliberately left a backend-shaped hole; 009a is that backend._
- **CDP screencast over VNC** — CDP is already JSON/WS and needs only Chromium + a port on the host (vs VNC's Xvfb + x11vnc + client). Tradeoff: renders only the browser viewport — exactly the scope. _Chose CDP because the scope is "the browser," not "the host desktop."_
- **Headless (`--headless=new`) on the host** — works without a display server (typical remote host). Headed-under-Xvfb is a documented later option. _Chose headless because CDP screencast needs no display and most hosts have none._
- **Forward (-L) tunnel, separate CDP port range (9200+)** — CDP lives on the host, so the Mac reaches it via -L (mirror of the -R MCP tunnel); distinct range so MCP + CDP tunnels coexist on one ControlMaster. _Chose a second range to avoid the 8990 MCP collision the sweep flagged._
- **Bridge lives in `agentum-server` (Rust)** — it already owns `host_runtime`, ControlMaster, and tunnels; the desktop already consumes runtime WS frames. _Chose server-side so the desktop stays a thin renderer (and a TUI could reuse it)._
- **One browser per worktree, deterministic tmux name** — reconnect = lookup, not relaunch; scopes the view to the worktree (spec requirement).

---

## Boundaries (what is NOT touched)

- **Local native browser webview** (`src/commands/browser_native.rs`, `NativeBrowserPagePane`) — unchanged; remote mode is a sibling branch keyed on `renderMode`.
- **agentum's own MCP wiring + reverse tunnel** (`mcp_provision.rs`, `sessions.rs` remote path) — unchanged; **009b** extends it for a browser MCP.
- **The agent launch path / `spawn_agent_into_pane`** — untouched; 009a uses a **separate** browser-launch path (no MCP/hooks/agent semantics), per the sweep's recommendation. 009a is user-driven only.
- Issue-posting, harness, Mac-local-app testing — out of scope.

### Files to touch (Developer)
- NEW `crates/agentum-server/src/host_browser.rs` — host launch + forward tunnel + CDP client + screencast pump + input dispatch + lifecycle map
- `crates/agentum-server/src/host_runtime.rs` — add `ensure_forward_tunnel` (ssh -L) reusing `ensure_reverse_tunnel`'s ControlPath/port-scan; new `REMOTE_CDP_PORT_BASE`
- `crates/agentum-tmux/src/ssh.rs` — add a `-L` forward command builder beside `ssh_control_forward_cmd` (`:341`)
- NEW `crates/agentum-server/src/routes/host_browser.rs` + register in `routes/mod.rs`, `lib.rs::router`
- `ui/src/shared/types.ts:551` — `BrowserPage.renderMode?: 'native' | 'remote-screencast'`
- `ui/src/store/slices/browser.ts` — wire `renderMode` + the existing `remoteBrowserPageHandlesByPageId`
- `ui/src/components/browser-pane/BrowserPane.tsx:745` — branch to `RemoteBrowserPagePane`; restore its WS subscription + input listeners (`:764–896`)
- `ui/src/components/browser-pane/remote-browser-frame-style.ts` — finish the stub
- Host preflight: `chromium --version` (offer `npx playwright install chromium` if missing) — reuse the `/api/hosts/{id}/readiness` pattern

---

## Risks

- **Live-view backend (now the main risk, downgraded)** — UI consumer exists; remaining work is the server CDP→protocol bridge + input fidelity. *Mitigation:* reuse the existing protocol + `RemoteBrowserPagePane`; throttle FPS, coalesce mouse-move, prioritize click/key; degrade to on-demand screenshots if the WS stalls.
- **Reconnect/re-attach** to the still-running browser (don't relaunch). *Mitigation:* deterministic per-worktree tmux name + host-side port marker; reconnect = lookup → re-tunnel → re-subscribe.
- **CDP port-bind race** (Chromium not yet listening). *Mitigation:* TCP healthcheck through the tunnel before "ready"; read the DevToolsActivePort file.
- **Missing Chromium / host heterogeneity.** *Mitigation:* preflight + **offer-install** (`npx playwright install chromium`), else **fail with a stated reason** — the spec criterion.
- **Cleanup** of headless Chromium. *Mitigation:* `kill_session` (sweep-confirmed), not `C-c`; isolate with `--user-data-dir=/tmp/...`.
- **Security** — CDP must bind **host loopback only** (`--remote-debugging-address=127.0.0.1`), reached solely via the authenticated SSH -L tunnel; never on a public interface.

---

## YAGNI

Re-activate dormant infra; one Chromium + one CDP screencast + one `ssh -L` helper + one new range. No VNC, no headed/Xvfb, no multi-browser/profile manager, no new frame protocol (the existing one is complete). Everything else is a later spec.
