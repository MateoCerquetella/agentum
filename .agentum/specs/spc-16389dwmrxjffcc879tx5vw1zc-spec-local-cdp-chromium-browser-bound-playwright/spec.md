---
schema: 1
id: SPC-16389DWMRXJFFCC879TX5VW1ZC
revision: 1
title: Spec: Local CDP-Chromium Browser + Bound Playwright MCP (009c-1)
source: legacy-import:ai/specs/009c-1-local-cdp-browser-bind/spec.md@sha256:0fe7d82952afbf42b1627034b0f6f0ffab5538452c973a27aa6509930a0b0c75
---

# Spec: Local CDP-Chromium Browser + Bound Playwright MCP (009c-1)

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

> # Spec: Local CDP-Chromium Browser + Bound Playwright MCP (009c-1)
>
> > Child of **009c** (PM split, 2026-06-18). The **first, unblocked** half: prove the
> > "one browser" model on the **local** substrate. An agent drives — over a real
> > Playwright MCP bound by CDP — the **same** Chromium-class browser the user is watching
> > in agentum, on this machine. 009c-2 generalises this exact wiring to an SSH host.
>
> ## Goal
>
> On the **local** machine, agentum displays a **CDP-controllable Chromium-class browser**
> (not WKWebView) for agent-driven / shared tabs, and a **Playwright MCP is bound to that
> exact browser instance** over `--cdp-endpoint`. An agent's `browser_navigate` / `click` /
> `snapshot` act on the **tab the user is watching** — one instance, not two.
>
> ---
>
> ## User Value
>
> **In one line:** "drive my browser" means *my* browser — locally, the agent automates the
> same window I'm watching instead of a hidden headless instance at `:8931` I can't see or
> take over.
>
> This is the unification proof on the easy substrate first: today the user sees a WKWebView
> (driven by `agentum_browser` via injected JS, no CDP) while the agent drives a separate
> hidden Playwright Chromium. 009c-1 makes the agent and the user share **one** CDP browser
> locally, de-risking the shared-context model before 009c-2 pays the remote-transport tax.
>
> ---
>
> ## Requirements
>
> - agentum exposes a browser surface for **agent-driven / shared** tabs backed by a
>   **CDP-controllable Chromium-class engine** (system Chrome / ungoogled-chromium /
>   Playwright-managed Chromium), launched with a known `--remote-debugging-port`. The user
>   **watches it live** (embed or CDP screencast — inherit 009a's decision, do not re-litigate).
> - `playwright_mcp.rs` gains a **`--cdp-endpoint` binding mode**: instead of spawning its own
>   hidden Chromium, the MCP **attaches** to agentum's displayed browser's CDP endpoint.
> - The **local** launch wiring (`mcp_provision.rs` + `sessions.rs` local path) wires the
>   **bound** Playwright MCP (replacing the hidden `:8931` headless instance for agent-driven
>   tabs). The lightweight `agentum_browser` MCP over WKWebView **stays untouched**.
> - **Shared-context lifecycle is defined:** who owns navigation / focus / tab lifecycle when
>   both the user and the agent can act on one CDP context. No race; a stated ownership model.
> - **Dependency handling:** detect a missing Chromium/Playwright **locally** and **offer to
>   install**; else **fail with a stated reason** — never a silent hang.
> - **MCP-agnostic seam preserved:** Playwright is the concrete reference binding; the binding
>   point is a config/registration seam, **not** a hardcoded sole option — but **no plugin
>   framework** (YAGNI).
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] Local agentum shows a browser tab backed by a **CDP-controllable** engine; the user watches it live.
> - [ ] A **Playwright MCP bound over `--cdp-endpoint`** to that browser: an agent's `browser_navigate`/`click`/`snapshot` produces a **user-visible change in the watched tab** (the unification proof — one instance, not two).
> - [ ] The old hidden `:8931` headless Chromium is **NOT spawned** for agent-driven tabs (binding mode active).
> - [ ] Missing local Chromium/Playwright is **detected with an offer to install**, else **fails with a stated reason** — no silent hang.
> - [ ] The Playwright binding sits **behind a seam** (config/registration point, not hardcoded as the only browser MCP) — verified by that seam, not a plugin framework.
> - [ ] WKWebView + `agentum_browser` lightweight path is **unchanged** (no regression for casual browsing).
>
> ---
>
> ## Dependencies
>
> - **`crates/agentum-server/src/playwright_mcp.rs`** — launches `@playwright/mcp` on `:8931`;
>   add the `--cdp-endpoint` binding mode (attach instead of spawn). Verified this session:
>   the server launches + listens, Playwright automation works locally.
> - **`crates/agentum-server/src/mcp_provision.rs`** — `McpProvision` / `McpServer`; wire the
>   bound MCP into the **local** agent launch (today: agentum MCP + `:8931` headless).
> - **`crates/agentum-server/src/routes/sessions.rs`** — `spawn_agent_into_pane` local path;
>   thread the CDP endpoint URL through.
> - **`crates/agentum-desktop/src/commands/browser_native.rs`** — the displayed browser tab;
>   the WKWebView→CDP-Chromium shift, **scoped to agent-driven / shared tabs only**.
> - **009a** — for the embed-vs-screencast live-view decision (local case may embed directly;
>   inherit the decision, don't re-litigate). NOT a hard build blocker for the local path.
>
> ---
>
> ## Risks
>
> - **WKWebView → CDP-Chromium is a large substrate change.** Mitigate: scope the CDP browser
>   to *agent-driven / shared* tabs first; keep WKWebView + `agentum_browser` for the
>   lightweight path. Don't rip out WKWebView wholesale.
> - **Shared one-CDP-context between user view and agent MCP must not race** — define
>   navigation/focus/lifecycle ownership explicitly (this is the core architect question).
> - **Live-view of a CDP Chromium** (embed vs screencast) — inherit 009a's decision.
> - **Over-abstraction** of "any browser MCP" — ship the Playwright CDP binding concretely,
>   leave a seam, no plugin framework (YAGNI).
>
> ---
>
> ## Out of Scope
>
> - The **SSH-host** case (→ 009c-2): tunnel, host-side screencast, persisted host results.
> - The verification *loop* / orchestration (008b); issue-posting (008a/008b); the harness (010).
> - Replacing WKWebView for *non-agent* casual browsing.
>
> ---
>
> ## Notes
>
> The hidden `:8931` headless Playwright instance is what we're replacing for agent-driven
> tabs — the binding mode points Playwright MCP at agentum's displayed browser's CDP endpoint
> instead. `agentum_browser` over WKWebView remains the lightweight path and is untouched.
> 009c-2 reuses this exact wiring shape on the host; resist a local special-case that drifts.
