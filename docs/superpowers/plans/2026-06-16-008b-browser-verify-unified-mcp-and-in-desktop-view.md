# 008b — Browser Verification: unified Playwright MCP, remote-host parity, in-desktop live view

> Extends **008a** (`skills/browser-verification-loop`). 008a defined the *loop*:
> drive each task in a real browser via Playwright MCP, capture a screenshot as
> mandatory evidence, post pass/fail to the linked issue, fail loud. This spec
> covers what 008a left open: **how the Playwright MCP browser is provisioned**
> (uniformly for local + tmux + remote), and **how the browser is shown inside
> the agentum desktop, next to the agent.**

## Goals

- **One provisioning mechanism** for Playwright MCP — local, tmux, and remote SSH
  hosts behave identically. No stdio-vs-HTTP branching to reason about.
- The loop works with the **agentum desktop closed** (local), and **continuously**
  inside a tmux/agent session (browser stays warm across tasks and agent restarts).
- On a **remote host**, the browser runs **on the host** (host network + host
  fingerprint), not on the Mac.
- Optionally render the **live browser in a pane next to the agent terminal,
  inside the agentum desktop.**

## Non-goals

- Driving agentum's built-in WKWebView with Playwright — impossible; Playwright
  cannot control the OS WebView. (This was the source of the macOS/WebKit confusion.)
- Embedding Playwright's Chromium as a native child of the Tauri window — unsupported.

### Hard constraint (verified) — provision BEFORE the agent starts

Claude Code (and Codex) load MCP servers **only at CLI startup**; there is no
in-session reload. (Claude docs: *"reads `.mcp.json` at startup… Exit and restart
the session after editing."*) Therefore the MCP server + config MUST be in place
**before the agent process launches** — you cannot make a *running* agent acquire
Playwright MCP. This kills 008a's "kick off the already-running active agent" model
unless that agent was itself launched with Playwright MCP. P1 is exactly this
provisioning-before-launch step.

Transport facts that pin the design:
- **HTTP, not SSE.** Claude Code: `{ "type": "http", "url": … }`. Codex supports
  **streamable-HTTP only** (no SSE). Playwright MCP `--port` serves at **`…/mcp`**.
- **Scope.** A *project*-scoped `.mcp.json` triggers a first-run approval prompt that
  blocks an unattended launch → provision at **local/user scope** (or pre-approve).

## Unified design (one mode everywhere)

1. **Transport — always HTTP.** Playwright MCP runs as a **persistent HTTP server**
   in its own tmux pane, identical local and remote. The agent config always points
   at the same shape:
   `{ "mcpServers": { "playwright": { "type": "http", "url": "http://127.0.0.1:<port>/mcp" } } }`
2. **Provision, then launch.** Before starting the verification agent, agentum:
   (a) ensures the Playwright-MCP pane exists on the target — the local tmux server,
   or the host's tmux over the existing SSH channel — starting it if missing:
   `npx @playwright/mcp@latest --port <port> --headless [--isolated | --user-data-dir <persist>]`;
   (b) writes the `http` MCP entry at **local scope**; (c) *then* launches/relaunches
   the agent so it picks the server up at startup. The watchdog keeps the pane alive
   → this is what makes it **continuous**.
3. **Agentum-closed (local).** A small `agentum` CLI helper performs the same
   provision step (start server on `<port>` if not reachable + write local-scope
   config); the user's next `claude`/`codex` launch has Playwright MCP. No desktop
   required — "closed is fine, but it works" holds, via the *same* provision logic.
4. **Remote parity (the 008b core).** The pane runs on the host (the agent session
   already lives there), so Chromium uses the host's browser + network. The HTTP port
   is forwarded to the Mac over SSH only for the optional live view.
5. **Evidence is the source of truth.** Screenshot per task, identical everywhere,
   posted to the issue (per 008a's strict-evidence contract). The live view is never
   required for a pass.

## In-desktop live view — "the browser next to the agent"

**Feasible via CDP screencast, not embedding.** Playwright MCP has no
"launch-with-debug-port" flag, but it has **`--cdp-endpoint`** (connect to an existing
Chrome). So the live-view flow is: *we* launch one Chrome with
`--remote-debugging-port=<cdp>`; Playwright MCP attaches to it via
`--cdp-endpoint=http://127.0.0.1:<cdp>`; and an agentum **webview pane beside the agent
terminal** opens a second CDP client on the same endpoint, calls `Page.startScreencast`,
and paints the JPEG frames to a canvas — a live mirror of exactly what Playwright drives.
One Chrome, driven by Playwright and mirrored by agentum.

- Same mechanism local and remote (forward `<cdp>` over the SSH channel for remote).
- Works with **headless** Chromium (screencast needs no display), so it also works
  on display-less remote hosts.
- Reuses agentum's existing terminal+browser split layout; the pane just points at
  the screencast surface instead of a URL. (Revives the previously-shelved screencast
  path for this one bounded use.)

## Phases (incremental)

- **P1 — provision-before-launch, local.** An `agentum` CLI helper that idempotently
  (a) starts the Playwright-MCP HTTP server on `<port>` if not reachable and
  (b) writes the `http` MCP entry at local scope. The verification launch calls it,
  *then* (re)starts the agent so it sees the server at startup; the 008a loop runs with
  screenshot evidence + issue post. Works with the desktop closed. Green locally.
- **P2 — remote parity (008b).** Same helper targets an SSH host's tmux; forward the
  HTTP port; acceptance = the browser reports the **host** identity, not macOS.
- **P3 — in-desktop live view.** We launch Chrome with `--remote-debugging-port`,
  Playwright MCP attaches via `--cdp-endpoint`, a screencast pane mirrors it beside
  the agent terminal.

## Test plan (verify later)

- **Unit:** provisioning selects the right target (local tmux vs host tmux), writes
  the correct `.mcp.json`, and idempotently reuses an already-live pane.
- **Local E2E (agentum closed):** run the skill on a sample task list against a local
  web app → screenshots captured, comment posted; MCP absent → fails loud, no green.
- **Remote E2E (acceptance for "uses the host's browser"):** same against an SSH host;
  a Device-Info page must report the **host** OS, not macOS 10.15.7.
- **Live view:** open the screencast pane, confirm frames track the agent's actions;
  verification still passes with the pane closed.

## Implementation status

- **P1 — DONE (foundation):** `ToolAdapter::mcp_args()` seam + `McpProvision` type in
  `agentum-executor`. Claude → `--mcp-config <file>` (additive); Codex → `-c
  mcp_servers.playwright.*`; all other tools → none. Unit-tested (executor `--lib`
  green). This is the per-tool half — pure, no I/O.
- **P1 — TODO (orchestration):** (1) a Playwright-MCP **server manager** (idempotent
  `ensure` → one shared HTTP server per machine/host, kept alive by the watchdog);
  (2) a **config-file writer** for Claude's `--mcp-config`; (3) **wire at the session
  launch site** — `ensure server → write file → argv.extend(adapter.mcp_args(&p))`
  before spawning; (4) integration test. Risk to verify live: exact Codex `-c` schema.

## Decisions resolved (this round)

- Transport = **HTTP** (`/mcp`). Codex is HTTP-only (no SSE); Claude Code uses
  `type:"http"`. No SSE anywhere.
- MCP must be provisioned **before** the agent launches (no mid-session reload).
- Provision at **local/user scope** to avoid the project-scope approval prompt that
  would block an unattended launch.

## Launch model — DECIDED: provision every agent session

Every agentum-launched agent session gets Playwright MCP (HTTP, local scope) wired at
**its own** launch, and the Playwright-MCP server is ensured up. Result: 008a's existing
"Launch → message the active agent" button works unchanged, because the active agent
already had Playwright MCP at startup. Cost accepted: every session carries a browser
MCP server even when not verifying — mitigate by sharing **one** server per machine/host
(idempotent ensure), not one per session.

## Open questions
- Port allocation per host: fixed vs ephemeral-advertised (reuse the
  `state/desktop-api.url` discovery-file pattern from the CLI auto-discovery work).
- `--isolated` (fresh each run) vs persistent `--user-data-dir` (keeps logins) default.
- Whether the CLI helper writes config via the existing
  `ui/src/shared/mcp-config.ts` formats or a Rust-side equivalent (desktop-closed
  path can't use the TS module).
