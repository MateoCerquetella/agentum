---
schema: 1
id: SPC-0XQ3QPAT36XSZSDW9KTRD1GD9E
revision: 1
title: Spec: Host Browser — Drive + Watch Live (009a)
source: legacy-import:ai/specs/009a-host-browser-live-view/spec.md@sha256:6c112f004054919edd3d09d52b208f18685d38eb12e7ca9fe740fcf178c14b4c
---

# Spec: Host Browser — Drive + Watch Live (009a)

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

> # Spec: Host Browser — Drive + Watch Live (009a)
>
> > Child of **009** (umbrella). The engine slice: prove a browser that **runs on the host**,
> > is **driven by the developer**, and is **watched live in agentum** — the hardest risk
> > (live-view transport) and the core pain relief. **009b** adds the agent/MCP path.
>
> ## Goal
>
> From a **remote-host session**, a developer launches a browser that **runs on the host** (pointed at the host app's `localhost:PORT`), runs their tests in it, and **watches it live in agentum** — scoped to the worktree — without dropping to a plain terminal.
>
> ---
>
> ## User Value
>
> **In one line:** browser testing against a host-running app happens **inside agentum**, so the developer stops being kicked back to plain SSH/tmux terminals — the everyday pain. Because the browser runs on the host, it survives the Mac sleeping by construction.
>
> ---
>
> ## Requirements
>
> - The browser process **runs on the host**, persistent (tmux/daemonized) so it survives Mac sleep / agentum close, reachable at the host app's `localhost:PORT`.
> - **Developer-driven:** the developer runs/drives their tests in it from inside agentum.
> - **Live view in agentum**, scoped to the worktree, via **streaming (CDP screencast / VNC) or port-forward** — not the current local-webview-only path.
> - **Reconnect:** reopening agentum re-attaches the live view to the **still-running** host browser (shows current state).
> - **Dependency handling:** detect a missing browser on the host and **offer to install** (e.g. `npx playwright install chromium`); else **fail with a stated reason** — no silent hang.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] From a host session, a browser launches **on the host** and loads the host app's `localhost:PORT` (reaches the **host's** localhost — no manual port-forward by the user)
> - [ ] The developer runs/drives browser tests there and **sees results**, never opening a plain terminal
> - [ ] The developer **watches the browser live** in agentum, scoped to the worktree, while the Mac is awake
> - [ ] Closing the lid / quitting agentum **does not kill** the host browser; reopening **re-attaches** the live view to its current state
> - [ ] If the host lacks the browser, agentum **offers to install** it; if it still can't start, it **fails with a stated reason** — no silent hang
>
> ---
>
> ## Dependencies
>
> - **`host_runtime`** (tmux/SSH on the host) — runs the persistent browser process on the host.
> - **A live-view transport** (CDP screencast / VNC / port-forward) — **new**; agentum's browser pane is a *local* webview and can't render a remote browser as-is (memory: native browser = local Tauri webview).
> - **Host prerequisites:** node/npx; Chromium (or chosen browser) installable.
> - Parent: **009**. No prior-spec blocker.
>
> ---
>
> ## Risks
>
> - **Live-view transport is the central technical risk** — the local webview can't render a host browser; needs screencast/VNC/port-forward. The single hardest piece (user has accepted this direction).
> - **Reconnect/re-attach** of the live view across agentum restarts — must point at the still-running host process, not spawn a new one.
> - **Host heterogeneity / missing deps** — mitigated by detect-and-offer-install + fail-loud.
>
> ---
>
> ## Notes
>
> **Out of scope (→ later / other specs):** agent-driven / browser-MCP path (**009b**); harness integration; issue-posting (008a/008b); testing **Mac-local** apps from the host browser (reverse network locality).
>
> **Grounding:** shipped build `33b748b` — on a host the launch wires agentum's MCP only (no Playwright), and `agentum_browser` drives a *local* webview (needs an open tab; `screenshot` unimplemented). This slice builds the missing host-side browser + the live-view transport.
