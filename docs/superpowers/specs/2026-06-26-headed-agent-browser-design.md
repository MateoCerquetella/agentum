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

### 4.2 Annotation channel over CDP
The WKWebView path posts annotations to `agentumgrab://annotation/add` (a Tauri scheme
handler). Headed Chrome has no such handler, so:
1. On each navigation, inject `INPAGE_ANNOTATE_JS` via **`Page.addScriptToEvaluateOnNewDocument`**
   (so it survives reloads), with the submit path swapped from the `Image`→scheme trick to a
   **CDP binding**: `Runtime.addBinding("__agentumAnnotation")`; the overlay calls
   `window.__agentumAnnotation(JSON.stringify(payload))`.
2. The server holds that CDP connection; on `Runtime.bindingCalled` it forwards the payload
   to the same annotation sink the WKWebView path uses (element context + comment + intent +
   optional `Page.captureScreenshot` of the element clip → the chosen agent).

This keeps **one** annotation format and reuses the picker/agent-send logic; only the
transport changes (CDP binding vs custom scheme).

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
- **Phase 1 (this spec):** headed launch mode, CDP annotation channel, launcher option, `#3`.
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
