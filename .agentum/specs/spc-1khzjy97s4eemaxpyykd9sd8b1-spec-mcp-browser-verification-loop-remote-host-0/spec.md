---
schema: 1
id: SPC-1KHZJY97S4EEMAXPYYKD9SD8B1
revision: 1
title: Spec: MCP Browser Verification Loop — Remote Host (008b)
source: legacy-import:ai/specs/008b-browser-verification-loop-remote/spec.md@sha256:e401db808bceaffd90eaaef77d6c33b87e366f9e7cb1359913ef42ee8d223378
---

# Spec: MCP Browser Verification Loop — Remote Host (008b)

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

> # Spec: MCP Browser Verification Loop — Remote Host (008b)
>
> > Child of **008** (umbrella). The risky slice: take the **008a** engine and make it
> > run on a **remote SSH host**, headless. Hard-depends on 008a.
>
> ## Goal
>
> A developer can run the **same** Agents & Automation browser-verification loop against a **remote SSH-host** session — identical trigger and result shape as local — with the **Playwright MCP** browser launching **headless** on the host.
>
> ---
>
> ## User Value
>
> **In one line:** the unattended browser-verification loop works on agentum's core use case — agents on a **remote host** — not just localhost, so "agent works while you're away" finally includes "*and verifies it in a real browser.*"
>
> ---
>
> ## Requirements
>
> - The **008a** loop runs against a remote SSH-host session via `host_runtime`, with an **identical launch and result shape**.
> - The **Playwright MCP** browser launches **headless** on the remote host and drives to completion.
> - **Loud failure** with a stated reason if the host can't start the browser (no Chromium / headless fail / no display).
> - **Network locality** is handled or clearly documented (a remote browser reaches the *host's* localhost).
> - *(Optional)* a host **preflight** that checks browser tooling before the loop starts.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] The **same** Agents & Automation launch produces the **same result shape** on a remote SSH-host session as on local (parity)
> - [ ] On the remote host, the Playwright MCP browser **launches headless** and the loop drives it to completion **unattended**
> - [ ] The loop **posts pass/fail to the linked issue** from the remote run, same as local
> - [ ] If the host can't start the browser tooling, the loop **fails with a stated reason** (e.g. "Chromium not installed / headless launch failed") — no silent hang
> - [ ] **Network locality is handled or documented:** a remote browser reaches the host's localhost; testing a local dev server requires a port-forward
>
> ---
>
> ## Dependencies
>
> - **008a** (engine + local + issue-posting) — **hard dependency**; 008b reuses its loop.
> - **Remote session execution** — `host_runtime` (tmux on the SSH host).
> - Remote host has **node/npx + Playwright browsers installable** (`npx playwright install --with-deps chromium`).
>
> ---
>
> ## Risks
>
> - **Headless Chromium won't start on the host** — missing deps / no display. The central risk; mitigated by the fail-loud criterion + optional preflight.
> - **Network locality** — a remote browser can't reach the developer's localhost; local dev servers need a port-forward.
> - **Host heterogeneity** — different distros / missing deps make "works on host" non-uniform across machines.
>
> ---
>
> ## Notes
>
> - Isolating the risky environment work here keeps **008a** shippable while this is hardened.
> - **Anchor:** reproduce 008a's result, unchanged, on a remote host.
