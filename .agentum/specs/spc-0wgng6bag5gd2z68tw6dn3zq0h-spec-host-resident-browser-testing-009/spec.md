---
schema: 1
id: SPC-0WGNG6BAG5GD2Z68TW6DN3ZQ0H
revision: 1
title: Spec: Host-Resident Browser Testing (009)
source: legacy-import:ai/specs/009-host-resident-browser-testing/spec.md@sha256:c45c3160a1674f4ab30c3d2aef4db3a0bfc191723b0912c0aebd1baa3e9f6b7b
---

# Spec: Host-Resident Browser Testing (009)

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

> # Spec: Host-Resident Browser Testing (009)
>
> > **STATUS: SPLIT (PM).** Full-vision capture. Ships as two one-screen child specs:
> > **009a** (host-resident browser you drive + watch live — the engine + the human journey)
> > → **009b** (agent-driven via a browser MCP on the host; **underpins 008b**). Take
> > 009a/009b through Architect → Developer; keep this file as the full-vision reference.
> > **008b decision:** 009 *underpins* 008b (008b = the verification-loop *use* of 009b's
> > agent+browser-MCP-on-host substrate), it does **not** absorb it.
>
> ## Goal
>
> From a **remote-host session** in agentum, a developer can **test a web app in a real browser that runs on the host** — driven by themselves or by an agent (browser MCP) — **watch it live** while the Mac is awake, and have it **keep running on the host** (with progress visible on return) when the Mac sleeps or agentum closes. No dropping to a plain terminal.
>
> ---
>
> ## User Value
>
> **In one line:** browser testing against a host-running app happens **inside agentum**, on the host — so the developer stops being kicked back to plain SSH/tmux terminals, and "survives laptop sleep" finally covers browser work too.
>
> Browser testing is everyday dev work. Today, on a host, it can't be done from inside agentum, so the developer abandons the tool and goes back to raw terminals. Cost of leaving it unsolved: **huge** — it's a core workflow, not an edge case. Persona: the **self-hoster** running remote agents (the project's primary user).
>
> ---
>
> ## Requirements
>
> - A **host-resident browser**: runs **on the host**, reaches the **host app's own `localhost:PORT`**, usable from a host session in agentum.
> - **Two drivers, one browser:** the developer can drive/run their tests against it, **and** an agent on the host can drive it via a **browser-automation MCP** (Playwright or another it installs — MCP-agnostic, not hardcoded).
> - **Live view (Mac awake):** the developer watches the host browser live in agentum, **scoped to the worktree**, via **streaming (CDP screencast / VNC) or port-forward** — not the current local-webview path.
> - **Continuity:** closing the lid or quitting agentum does **not** stop the host browser work; reopening agentum **re-surfaces the progress** (results/screenshots persisted on the host).
> - **Dependency handling:** when the host lacks the browser the MCP needs, **detect it and offer to install** (e.g. `npx playwright install chromium`) — don't fail silently.
> - **Loud failure:** if the browser tooling can't start on the host, report a **stated reason**.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] From a host session, a browser opens against the host app's `localhost:PORT` and **loads** (reaches the **host's** localhost — no manual port-forward by the user)
> - [ ] The developer runs their browser tests there and sees **pass/fail**, never opening a plain terminal
> - [ ] An **agent on the host** can drive a browser MCP (Playwright or one it installs) and **return a result**
> - [ ] The developer can **watch it live**, scoped to the worktree, while the Mac is awake
> - [ ] Close the lid / quit agentum → the host browser work **continues**; reopen agentum → the **accrued progress is shown** (persisted results/screenshots = the host-execution proof)
> - [ ] If the host **can't start** the browser tooling, agentum **offers to install** the missing browser, or **fails with a stated reason** — no silent hang
>
> ---
>
> ## Dependencies
>
> - **`host_runtime`** (tmux/SSH on the host) — session execution on the host.
> - **agentum's own MCP server + reverse-tunnel wiring** (new since 008) — the agent-on-host MCP path. **Gap:** today agentum wires only its *own* MCP on a host, **not a browser MCP** — a browser MCP must be provisioned **on the host**.
> - **A live-view transport** (CDP screencast / VNC / port-forward) — new; agentum's browser pane is a *local* webview and can't render a remote browser as-is.
> - **Host prerequisites:** node/npx present; Chromium (or the MCP's browser) installable.
> - **Prior art:** **008b** (remote browser-verification parity) assumed "Playwright headless on the host" — this spec supplies the concrete host-resident substrate 008b needs; PM/Architect to decide whether 009 **absorbs or underpins** 008b. **008a** (local loop) is unaffected.
>
> ---
>
> ## Risks
>
> - **Live-view of a host browser is the hard part** — needs streaming/port-forward, not the local-webview path. Accepted by the user; still the central technical risk.
> - **Host heterogeneity / missing deps** (node, Chromium, distro differences) — mitigated by **detect-and-offer-install** + fail-loud.
> - **Persisting progress on the host + reattach on reopen** — a new persistence surface (analogous to the per-session pane logs); without it, "see the progress on return" doesn't hold.
> - **Agent reports green without actually driving** (carry from 008) — the **persisted screenshots/results** are the hard evidence.
> - **Network locality** — a host browser reaches the *host's* localhost (correct for host apps); testing a **Mac-local** app from the host browser is the reverse case and is **out of scope**.
>
> ---
>
> ## Notes
>
> **Out of scope this round (parked):**
> - **Harness-loop integration** — explicitly de-scoped per the user ("forget about harness"); the harness is a *downstream beneficiary*, not this spec's driver.
> - **Issue-posting to GitHub/Linear** — that's 008a/008b's concern; not required here.
> - **Testing Mac-local apps from a host browser** (reverse network locality).
> - **Locking to one browser MCP** — stay MCP-agnostic; Playwright is the reference, not the requirement.
>
> **Grounding:** written after directly verifying the shipped build (commit `33b748b`): on a host, the launch wires agentum's MCP only (no Playwright); `agentum_browser` drives a *local* webview (needs an open tab; `screenshot` unimplemented). That reality is exactly the gap this spec closes.
