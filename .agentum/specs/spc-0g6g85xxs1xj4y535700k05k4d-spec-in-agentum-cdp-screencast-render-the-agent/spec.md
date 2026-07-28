---
schema: 1
id: SPC-0G6G85XXS1XJ4Y535700K05K4D
revision: 1
title: Spec: In-agentum CDP Screencast — render the agent's browser INSIDE agentum (009c-3)
source: legacy-import:ai/specs/009c-3-in-agentum-screencast/spec.md@sha256:2173bcad55bf213dbccf3de7f258f8e47135f75cc09e20c57b2c70bc450c4245
---

# Spec: In-agentum CDP Screencast — render the agent's browser INSIDE agentum (009c-3)

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec: In-agentum CDP Screencast — render the agent's browser INSIDE agentum (009c-3)
>
> > Born from user feedback on 009c-1 (2026-06-18): the agent-driven CDP browser must render
> > **inside agentum's browser pane**, NOT as a separate OS window — and the same way for
> > **local and SSH hosts**. This is the heavy UI slice the 009c-1 architecture explicitly
> > deferred. It supersedes 009c-1's "headed OS window" live-view decision.
>
> ## Goal
>
> The agent-driven CDP-Chromium (headless, local or on a host) is **rendered live inside
> agentum's browser pane** via CDP screencast, and the user can interact with it there
> (click/type/scroll/navigate). The agent drives the **same** browser over the bound Playwright
> MCP. No separate OS window; no manual env-var ceremony.
>
> ## User Value
>
> "Everything inside agentum, even for hosts." The browser the user watches and the browser the
> agent drives are one instance, shown in agentum's own pane — local or remote, identical UX.
>
> ## Requirements
>
> - A **server-side CDP→screencast bridge** on the embedded agentum-server: connect to the
>   headless CDP browser (local port, or host port over the 009a `ssh -L` tunnel), run
>   `Page.startScreencast`, and stream frames to the pane in the **existing `0x62` binary
>   protocol** (`shared/browser-screencast-protocol.ts`, version 1).
> - **Input + navigation back-channel**: the pane's mouse/keyboard/scroll/nav events →
>   CDP `Input.dispatchMouseEvent` / `Input.dispatchKeyEvent` / `Page.navigate` etc.
> - **Client**: activate `RemoteBrowserPagePane` (reuse its `0x62` decoder, `<img>` render, and
>   input serializer) but point it at the **embedded-server WS** (thin-shell aligned) instead of
>   the stubbed native `runtime_environments_subscribe`. Flip `BrowserPane` to select it for
>   agent-driven pages.
> - **Headless local browser**: with screencast rendering, `cdp_browser` launches Chromium
>   **headless** (no OS window) — agentum renders it.
> - **Local + host = one bridge**: only the CDP endpoint location differs (local port vs tunneled).
> - **Gating becomes a toggle**, not a manual env var (replace the `AGENTUM_BROWSER_VERIFY` hand-set).
>
> ## Acceptance Criteria
>
> - [ ] An agent-driven browser renders **inside agentum's pane** (no separate OS window); the user sees it live.
> - [ ] An agent `browser_navigate`/`click` is visible in that pane in real time (same instance).
> - [ ] The user can click/type/scroll in the pane and the CDP browser responds.
> - [ ] Works the same for a browser on an SSH host (CDP over the 009a tunnel).
> - [ ] No manual env var — a Settings toggle (or sensible default) enables it.
> - [ ] Frame bytes match the existing `0x62` protocol (client decoder unchanged).
>
> ## Dependencies / seams
>
> - **Frame protocol** (FIXED contract): `crates/agentum-desktop/ui/src/shared/browser-screencast-protocol.ts`
>   — kind `0x62`, version 1, 16-byte header `[kind,ver,opcode,format,seq u32LE,mdlen u32LE,reserved u32]`
>   + metadata JSON + image bytes. Opcode `0x01` = Frame; format 1=jpeg, 2=png.
> - **NEW** `crates/agentum-server/src/cdp_screencast.rs` — CDP client (tokio-tungstenite) + frame encoder + input/nav dispatch.
> - **NEW** `crates/agentum-server/src/routes/cdp_screencast.rs` — axum WS route bridging pane ↔ CDP.
> - `cdp_browser.rs` — switch to headless when screencast renders it.
> - **Client**: `RemoteBrowserPagePane` (`components/browser-pane/BrowserPane.tsx` ~764), the input
>   serializer (`remote-browser-keyboard.ts`), the handle store (`store/slices/browser.ts`), and the
>   pane-selection point (~746) — rewire from `api.runtimeEnvironments.subscribe` to the embedded-server WS.
> - **009a tunnel** for the host case (forward `ssh -L` to the host CDP port).
>
> ## Risks
>
> - **Biggest slice in 009**; the native `runtime_environments_*` transport the dormant UI used is
>   stubbed, so we route via the embedded server instead (thin-shell aligned) and rewire the pane.
> - **Client verification needs the npm/Vite build env** (unreliable headless; sits near foreign WIP) —
>   the Rust engine is unit/integration-testable; the pane rewire needs a UI build to confirm.
> - CDP screencast frame cadence / ack (`Page.screencastFrameAck`) must be handled or frames stall.
> - Input coordinate mapping (CSS viewport → device pixels) must match the client's normalization.
>
> ## Out of scope
>
> - Replacing WKWebView for casual (non-agent) browsing — the lightweight `agentum_browser` path stays.
> - The harness/orchestration; issue-posting.
