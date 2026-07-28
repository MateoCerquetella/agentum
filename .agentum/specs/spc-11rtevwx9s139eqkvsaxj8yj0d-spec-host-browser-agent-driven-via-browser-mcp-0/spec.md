---
schema: 1
id: SPC-11RTEVWX9S139EQKVSAXJ8YJ0D
revision: 1
title: Spec: Host Browser — Agent-Driven via Browser MCP (009b)
source: legacy-import:ai/specs/009b-host-browser-agent-mcp/spec.md@sha256:dbddfa13971028a1de1dcc2864685a9fd3f77860ec84410b0c0a45df6bf9e84c
---

# Spec: Host Browser — Agent-Driven via Browser MCP (009b)

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

> # Spec: Host Browser — Agent-Driven via Browser MCP (009b)
>
> > Child of **009** (umbrella). Adds the **agent** journey on top of **009a**'s host browser:
> > a browser-automation **MCP that runs on the host**, so an agent verifies its own work in
> > a real browser unattended. **Underpins 008b** (the verification loop is a *use* of this).
>
> ## Goal
>
> From a **remote-host session**, an **agent on the host** drives a browser-automation **MCP** (Playwright or one it installs) against the host app and **returns results** (screenshots/assertions) that persist on the host and re-surface in agentum on return.
>
> ---
>
> ## User Value
>
> **In one line:** agents verify their own work in a real browser **on the host**, unattended — so "agents work while you're away" finally includes browser verification, and the result is waiting for you in agentum when you reopen it.
>
> ---
>
> ## Requirements
>
> - A browser-automation **MCP runs on the host** and is reachable by the host agent. *(Gap today: the remote launch wires only agentum's **own** MCP — no browser MCP — see `sessions.rs` remote path.)*
> - **MCP-agnostic:** Playwright is the reference; support whichever browser MCP the user installs — don't hardcode.
> - The agent drives the MCP against the host app's `localhost:PORT` and **returns a result**.
> - **Results persist on the host** (screenshots/assertions) and **re-surface in agentum on reopen** — the host-execution proof.
> - **Dependency handling:** detect a missing browser/MCP on the host and **offer to install**; else **fail with a stated reason**.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] An agent in a host session can call a browser MCP (Playwright or installed) that **runs on the host** and drive the host app's `localhost:PORT`
> - [ ] The agent **returns a result** (e.g. screenshot/assertion) that is **persisted on the host**
> - [ ] On reopening agentum, the **persisted results are visible** (proof the run happened on the host)
> - [ ] The browser-MCP wiring is **not hardcoded to Playwright** (agnostic)
> - [ ] If the host lacks the browser/MCP, agentum **offers to install** it; else **fails with a stated reason** — no silent hang
>
> ---
>
> ## Dependencies
>
> - **009a** — host browser substrate (hard dependency); 009b drives the same host browser.
> - **agentum MCP provisioning + reverse tunnel** (`mcp_provision.rs`, `sessions.rs` remote path) — **extend** it to wire a *browser* MCP on the host, not only agentum's own.
> - **`host_runtime`**; host node/npx + Chromium installable.
>
> ---
>
> ## Risks
>
> - **New host-side MCP provisioning** — today only agentum's own MCP is wired remotely; a browser MCP on the host is net-new wiring (which transport, who starts it, lifecycle).
> - **Over-abstraction** of "any browser MCP" — YAGNI; ship Playwright concretely, leave a seam, don't build a plugin framework.
> - **Agent reports green without driving** — the **persisted screenshots/results** are the hard evidence (don't trust the self-report).
> - **Host heterogeneity / missing deps** — mitigated by detect-and-offer-install + fail-loud.
>
> ---
>
> ## Notes
>
> **Relationship to 008b:** 008b (remote browser-verification loop) should **depend on 009b** instead of its original "Playwright headless on the host" assumption — 008b becomes the *loop/orchestration use* of this substrate.
>
> **Out of scope:** the loop/orchestration + stop-condition (008b); issue-posting (008a/008b); harness; the live-view UI (009a).
