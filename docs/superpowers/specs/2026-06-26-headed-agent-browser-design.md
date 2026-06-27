# Headed agent browser — real Chrome, agent-driven, persistent (Option A)

**Date:** 2026-06-26
**Status:** Design — Phase 1 spec drafted, implementing.
**Area:** `crates/agentum-server/` (CDP launch + driver + annotation channel),
`crates/agentum-desktop/ui/` (browser launcher UI).

## 1. Summary

The in-app **agent browser** today is a headless Chromium streamed into a `<canvas>`
via `Page.startScreencast` (JPEG frames). That stream is the bottleneck: on a hi-DPI /
big screen a sharp 2× capture means 4× pixels per frame → laggy, and the frame-size cap
makes it chunky when stretched. It kills developer UX.

**The screencast only exists because Chrome runs `--headless` (no window).** The agent
does **not** depend on headless — every `cdp_driver` op (navigate/click/fill/snapshot/
node_at_point) is browser-agnostic and works identically against a *headed* browser.

So: run a **real headed Chrome window** driven by CDP. The agent controls it exactly as
today; the user sees **real Chrome — native, sharp, zero stream lag**; it persists; and
annotations are injected over CDP. This is a return to the original `009c-1` headed design
(superseded by `009c-3` headless-in-pane), now reusing all the modern CDP/tmux/annotation
infra.

```
"Open Browser"              → native WKWebView   (fast local view, NOT agent-driven)
"Open Browser (persistent)" → HEADED Chrome + CDP (agent-driven, real Chrome, persistent)
```

LOCAL → headed (no streaming). The **screencast stays only for remote/SSH** browsers
(where the browser is on another machine and streaming is unavoidable; a future WebRTC
upgrade is out of scope here).

## 2. Goals / Non-goals

**Goals**
- A persistent browser the **agentum MCP** drives via CDP (unchanged `cdp_driver`).
- **Native Chrome UX** (real window, no streamed frames).
- **Annotations** — point at an element in the real Chrome → the agent receives the
  element context + comment + intent (reusing the existing `INPAGE_ANNOTATE_JS` logic).
- Per-worktree isolation + tmux persistence (reused as-is).
- Configurable screencast device scale for the remaining remote case (`#3`).
- A UI choice between the two browsers (`#4`).

**Non-goals (this spec)**
- Embedding the Chrome window *inside* the agentum pane. macOS cannot cleanly reparent
  another process's window; true in-pane embedding needs CEF (Phase 3). Phase 1 ships a
  **separate, linked Chrome window**.
- Window-following (the Chrome window tracking the agentum pane) — Phase 2.
- WebRTC streaming for remote — future.
- Removing the screencast — it remains for remote/SSH.

## 3. Reused infrastructure (why this is cheap)

| Infra | File | Reuse |
| --- | --- | --- |
| CDP driver ops | `cdp_driver.rs` | as-is — browser-agnostic, routes to any CDP endpoint |
| Browser launch + tmux persistence | `cdp_browser.rs` | add a **headed** launch variant |
| Per-worktree isolation (port+tmux+profile) | `cdp_browser.rs` | as-is |
| Annotation overlay JS | `browser_native.rs::INPAGE_ANNOTATE_JS` | inject over CDP instead of WKWebView eval |
| MCP routing | `routes/mcp.rs::tool_browser` | as-is |

The **only genuinely new** pieces are: a headed launch mode, an annotation channel that
does not depend on the Tauri `agentumgrab://` scheme (headed Chrome has no Tauri scheme
handler), and the launcher UI option.

## 4. Design — Phase 1

### 4.1 Headed launch mode (`cdp_browser.rs`)
`build_chrome_argv` / `remote_chrome_launch_script` gain a **mode**:
- `Headless` (current): `--headless=new … --force-device-scale-factor=<scale>` (screencast).
- `Headed` (new): drop `--headless=new`; add `--app=about:blank` (chrome-less app window)
  and a sane `--window-size`/`--window-position`. **No** `--force-device-scale-factor`
  (a real window renders natively at the display scale).

Selection: the persistent-browser launch path (`ensure_local_cdp_browser*`) takes a mode
arg. `agentum_browser` "open"/launch for the persistent surface requests `Headed`. The
screencast subscribe path keeps requesting `Headless`. Tmux session naming gets a `-h`
suffix for headed so a headed and a headless browser never collide on one profile/port.

### 4.2 Annotation channel over CDP (implemented — Phase 1b)
The WKWebView path posts annotations to `agentumgrab://annotation/add` (a Tauri scheme
handler). Headed Chrome has no such handler. Rather than hold a persistent CDP connection
for `Runtime.bindingCalled` (the one-shot `cdp_driver` model doesn't), the headed overlay
**beacons over HTTP to agentum's own loopback server** — simpler and connection-free:

1. **Inject** — a new `cdp_driver` op `annotate` (`cdp_annotate`) injects
   `ANNOTATE_OVERLAY_JS` (the WKWebView overlay, byte-identical payload) via
   `Runtime.evaluate`. It is **not** gated by `AGENTUM_BROWSER_ALLOW_EVAL` (a fixed trusted
   script, not caller code). The overlay's `submit()` is the only change vs the WKWebView
   one: `fetch(ANNOTATE_URL, {method:'POST', body, mode:'no-cors', keepalive:true})` (no
   custom-scheme `Image`). `mode:'no-cors'` means no JSON content-type header, so the route
   reads a raw string body.
2. **Receive + broadcast** — `POST /api/cdp-browser/annotation/add` parses the raw body and
   rebroadcasts it on the existing `/api/events` bus as a `browser.annotation` event
   (`state.bus.send`). Reachable on the embedded loopback server (no_auth); the page can't
   carry a token.
3. **Surface** — `arm` is `POST /api/cdp-browser/annotate {worktreeId}` (resolves the
   worktree's headed `cdpPort`, builds the loopback `annotateUrl` from `state.api_base_url`,
   runs the `annotate` op). The desktop subscribes via `openBrowserAnnotationStream`
   (`useServerBrowserAnnotations`, mounted by `App.tsx` next to `useIpcEvents`) and surfaces
   each annotation as a toast with a **Copy for agent** action
   (`formatHeadedAnnotationForAgent`). The headed window has no in-app tab, so it can't reuse
   the per-tab WKWebView tray; auto-routing the prompt to a specific worktree agent (vs the
   clipboard hand-off) is a Phase 1b+ refinement.

UI entry: an **"Annotate persistent browser"** launcher item (`annotatePersistentBrowser`
command). The annotation **payload shape stays identical** to the WKWebView path.

### 4.3 Pane / launcher UI (`#4`)
The browser launcher (new-tab surface) offers two entries:
- **Open Browser** → existing native WKWebView pane.
- **Open Browser (persistent)** → launches/attaches the headed Chrome (separate window) and
  shows a thin control pane in agentum: address bar (drives `browser.goto` over CDP), back/
  forward/reload, an **Annotate** toggle (arms the injected overlay), and a "Chrome window"
  affordance (focus/raise the real window). No canvas, no stream.

### 4.4 Configurable device scale (`#3`)
`cdp_device_scale()` (env `AGENTUM_CDP_DEVICE_SCALE`, default 2, clamp [1,4]) feeds the
**screencast/headless** `--force-device-scale-factor`. Lets a remote/SSH screencast trade
sharpness for speed. (Already implemented.)

## 5. Phasing
- **Phase 1a (shipped first):** headed launch mode (a real Chrome window with full UI —
  the user navigates directly; **normal window, not `--app`**, so no agentum control pane
  is needed), MCP prefers the headed browser, the `POST/DELETE /api/cdp-browser/headed`
  route, the "Open Browser (persistent)" launcher entry, and `#3`. This alone removes the
  laggy screencast for the local agent browser — the primary pain.
- **Phase 1b (implemented — branch `feat/headed-browser-annotations`):** the annotation
  channel via injected overlay → loopback `fetch` beacon → `/api/events` broadcast →
  desktop toast (see §4.2). Server backbone is unit-tested; the injected overlay + the
  end-to-end toast need GUI verification on a real Chrome window before release. The
  screencast annotation path (`AgentBrowserPickerOverlay` + `node_at_point`) still works for
  the remote/SSH screencast surface.
- **Phase 2:** window-following so the Chrome window tracks the agentum pane bounds.
- **Phase 3 (optional):** CEF for true in-pane Chromium embedding.

## 6. Acceptance criteria (Phase 1)
- [ ] "Open Browser (persistent)" launches a **real headed Chrome window** (not headless).
- [ ] The `agentum_browser` MCP drives that window (navigate/click/fill/snapshot) unchanged.
- [ ] The browser **persists** across an app restart (tmux), per worktree.
- [ ] Annotating an element in the headed Chrome delivers element context + comment + intent
      to the agent (over the CDP binding), same payload shape as the WKWebView path.
- [ ] No screencast frames are streamed for the local persistent browser (native UX).
- [ ] `AGENTUM_CDP_DEVICE_SCALE` tunes the remaining (remote/screencast) capture scale.
- [ ] `cargo test -p agentum-server` green; desktop builds.

## 7. Risks
- Headed Chrome steals focus / opens as a separate window — acceptable for Phase 1 (real
  Chrome window UX is the point); Phase 2 integrates it.
- CDP binding injection timing — use `addScriptToEvaluateOnNewDocument` so it's present
  before page scripts and survives navigation.
- A pre-existing headless browser on the per-worktree port must not be reused for the headed
  surface — the `-h` tmux/profile split prevents collision.
