# 009c-3 — Client-side screencast contract (mapped 2026-06-18)

> The dormant in-agentum screencast UI already exists; the server bridge must match THIS
> contract. Captured so the build session needn't re-explore. All paths under
> `crates/agentum-desktop/ui/src/`.

## Frame wire protocol (FIXED) — `shared/browser-screencast-protocol.ts`
- kind `0x62`, version `1`, 16-byte header, all u32 **little-endian**.
- `[0]=0x62 [1]=1 [2]=opcode(0x01=Frame) [3]=format(1=jpeg,2=png) [4..8]=seq u32 [8..12]=mdlen u32 [12..16]=reserved(=0)] + metadataJSON + imageBytes`
- **Decoder rejects the frame** if reserved≠0 OR metadata isn't a finite-number JSON object → always send a valid object (empty `{}` ok).
- Metadata keys (camelCase): `offsetTop, pageScaleFactor, deviceWidth, deviceHeight, imageWidth, imageHeight, scrollOffsetX, scrollOffsetY, timestamp`.
- ✅ Rust encoder already built + matches this exactly: `crates/agentum-server/src/cdp_screencast.rs::encode_frame` (3 passing tests).

## How the pane subscribes today (the part to REWIRE) — `components/browser-pane/BrowserPane.tsx`
- `RemoteBrowserPagePane` (~line 764), exported DISABLED as `LegacyRemoteBrowserPagePane`; active pane is `NativeBrowserPagePane` (WKWebView). Pane selection ~line 746 (hardcoded to native).
- Subscribes via `api.runtimeEnvironments.subscribe({ selector: environmentId, method: 'browser.screencast', params, timeoutMs }, { onResponse, onBinary, onError, onClose })` (~line 1523).
  - `params`: `{ worktree:'id:<wt>', page:<pageId>, format:'jpeg', quality:70, maxWidth:3840, maxHeight:2160, viewportWidth, viewportHeight, deviceScaleFactor(1-2), everyNthFrame:2 }`.
  - `onBinary(bytes)` → `updateStreamFrame()` → `decodeBrowserScreencastFrame` → `<img src=ObjectURL(blob)>` (~line 2368-2404).
  - `onResponse` gets JSON control: `{type:'ready', subscriptionId, browserPageId, format, tab}` | `{type:'end',…}` | `{type:'error',message}` (`shared/runtime-types.ts` ~490-539).
  - Capability gate: requires `status.capabilities` includes `'browser.screencast.v1'` (~line 1505; declared in `shared/protocol-version.ts` ~27).
- **REWIRE TARGET**: `api.runtimeEnvironments.*` dispatches to Tauri `invoke('runtime_environments_subscribe')` (`tauri/runtimeEnvironments.ts`), which is a **STUB** (`crates/agentum-desktop/src/commands/runtime.rs:66,71`). Point the pane at the **embedded agentum-server WS** instead (see `ui/src/runtime/agentum-server-client.ts` for the embedded-server client; other routes use `apiUrl`/`wsUrl`). Keep the `0x62` decoder, `<img>` render, and input serializer as-is.

## Input back-channel — `components/browser-pane/remote-browser-keyboard.ts` + BrowserPane.tsx
- Emits RPCs: `browser.mouseMove{page,x,y}`, `browser.mouseDown{page,button:'left'|'middle'|'right'}`, `browser.mouseUp{page,button}`, `browser.mouseWheel{page,dx,dy}`, `browser.keypress{page,key}`, `browser.viewport{page,width,height,deviceScaleFactor,mobile}`, plus nav `browser.goto/back/forward/reload{page,url?}`, tabs `browser.tabCreate{url}→{browserPageId}`, `browser.tabClose{page}`, `browser.tabShow{page}→{tab}`.
- Coords normalized to viewport: `x = round(((clientX-rectLeft)/rectWidth)*viewportWidth)` (~line 1257-1284). Server maps → CDP `Input.dispatchMouseEvent`/`dispatchKeyEvent`, `Page.navigate`.
- Keyboard helpers: `getRemoteBrowserKeypressKey(event)` (char or Enter/Backspace/Delete/Tab/Escape/Arrows), `getRemoteBrowserKeyboardShortcut(event)` (e.g. "Meta+r").

## Handle store — `store/slices/browser.ts` (~92-101, 1312-1336)
- `remoteBrowserPageHandlesByPageId: Record<string,{environmentId,remotePageId}>`; `setRemoteBrowserPageHandle`/`removeRemoteBrowserPageHandle`. Already present; use to mark a page as screencast-rendered + flip pane selection.

## Server bridge to build (matches the above)
- WS route on embedded agentum-server (axum WS, like `routes/events.rs`/`harness.rs`). Path TBD e.g. `/api/cdp-browser/screencast?page=…&token=…`.
- On connect: connect to the headless CDP browser (local port `cdp_browser::port()` = 9300, or host port over ssh -L), `Target.attach`/`Page.enable`, `Page.startScreencast{format,quality,maxWidth,maxHeight,everyNthFrame}` → on each `Page.screencastFrame{data(base64),metadata,sessionId}`: `encode_frame()` → send binary; then `Page.screencastFrameAck{sessionId}` (REQUIRED or frames stall).
- Inbound pane messages → CDP `Input.dispatchMouseEvent`/`dispatchKeyEvent`/`Page.navigate`.
- Advertise `browser.screencast.v1` capability wherever the client checks `status.capabilities`.
