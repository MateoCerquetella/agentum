---
schema: 1
id: SPC-1VPMB0J2YR03AESYS8BFBTT7M4
revision: 1
title: Spec: MCP Browser Verification Loop
source: legacy-import:ai/specs/008-mcp-browser-verification-loop/spec.md@sha256:bb2efc0abd5ca7c3d3b786ec8a79dbfe13892de46e7d142f6325c01f4987197d
---

# Spec: MCP Browser Verification Loop

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

> # Spec: MCP Browser Verification Loop
>
> > **STATUS: SPLIT.** This is the parent/umbrella capture of the full vision. It ships as
> > two one-screen child specs: **008a** (engine + local + issue-posting) → **008b**
> > (remote-host parity). Take 008a/008b through Architect → Developer; keep this file as
> > the full-vision reference.
>
> ## Goal
>
> A developer can **launch**, from the **Agents & Automation** surface, an autonomous agent loop that drives the **Playwright MCP** browser to verify its task list — running identically on a local or a remote-host session — and **posts pass/fail back as a comment on the tracked GitHub/Linear issue**.
>
> ---
>
> ## User Value
>
> **In one line:** agents on a remote host verify their own work in a real browser and report pass/fail to the issue — unattended — instead of the developer doing that check by hand on localhost.
>
> Autonomous agent loops that verify work in a real browser already work *locally* (proven: Ralph Loop + Playwright MCP completed 10 tasks unattended, perfectly). But agentum's whole promise is **agents on remote hosts running while you're away** — and that exact browser-verification loop goes blind the moment the agent is headless on a remote box.
>
> Closing that gap turns "agent runs unattended" from *writes code* into *writes code **and verifies it in a real browser, on the host, by itself**.* It removes the manual, localhost-only check the developer does by hand today, and serves every persona — solo dev, multi-agent power user, and self-hoster on a remote box — because the result lands where work is tracked (the issue), not in pane scrollback you'd have to babysit.
>
> ---
>
> ## Requirements
>
> - A launch point on **Agents & Automation** (an "agentic skill" / action) that starts an autonomous browser-verification loop, bound to a session/repo and a linked issue.
> - The loop drives the **Playwright MCP** browser to execute **and** verify each task **entirely in-browser** — both "run the suite" and "agent autonomously checks what it built" collapse into this, since both are pure-browser work via MCP.
> - **One engine, two environments:** the same launch and the same result shape must work on a **local** session and a **remote SSH-host** session (`host_runtime`). On the host the browser runs **headless**.
> - **Loud failure:** if the Playwright MCP browser cannot start on the target host, the loop reports a stated reason instead of hanging silently.
> - **Result destination:** on completion (or per task), the loop posts pass/fail as a **comment on the linked GitHub/Linear issue**, via agentum's existing integration.
> - **Bounded autonomy:** the loop has an explicit stop condition (max iterations / task budget) so it can run unattended without running away.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] From **Agents & Automation**, a developer can launch an autonomous browser-verification loop bound to a session/repo and a tracked issue
> - [ ] The loop drives the **Playwright MCP** browser to execute and verify every task **in-browser** (no stubbed or non-browser shortcut)
> - [ ] The loop produces the **same launch and same result shape** on a **local** session and a **remote SSH-host** session; on the host the browser launches **headless** and drives unattended to completion
> - [ ] If the browser tooling cannot start on the target host, the loop **fails with a stated reason** (e.g. "Chromium not installed / headless launch failed") — no silent hang
> - [ ] On completion (or per task), the loop **posts the pass/fail result as a comment** on the linked **GitHub / Linear** issue
> - [ ] The loop has an **explicit stop condition** (max iterations or task budget) and halts there without human intervention
>
> ---
>
> ## Dependencies
>
> - **External issue-tracker integration (GitHub / Linear)** — already present in agentum (gh/glab CLI + Linear token store); used to post the result comment. *Not fully under our control: auth, rate limits, correct-issue targeting.*
> - **Playwright MCP config support** — the existing `.mcp.json` inspector (`crates/agentum-desktop/ui/src/components/settings/McpConfigSection.tsx`) plus the agent CLIs that read it and launch the server. agentum does not run the MCP server itself — the agent CLI in the tmux pane does.
> - **Remote session execution** — `host_runtime` (tmux on the SSH host) so the loop + browser run on the remote machine.
> - **Autonomous-loop pattern** — conceptually the Ralph-style loop already exercised in spec 007's execution.
> - Prior specs: no hard blocker; **007** (desktop-settings cleanup + integrations surface) is adjacent, since the Agents & Automation surface lives there.
>
> ---
>
> ## Risks
>
> - **Headless browser won't start on the remote host** — missing Chromium / system deps / no display. The single most likely real-world failure. *Mitigation:* the "fail loudly with a reason" criterion + an optional host preflight (`npx @playwright/mcp --headless --version`).
> - **Agent reports green without actually driving** (silent failure / hallucinated `10/10`). *Open question for the Architect:* how much to trust the agent's self-report vs. requiring hard evidence (screenshot/trace per task) before posting the comment.
> - **Unattended loop runs away** — infinite retries, runaway token cost, or wedging the remote host. *Mitigation:* explicit stop condition / task budget (acceptance criterion).
> - **External tracker dependency** — GitHub/Linear API/CLI auth, rate limits, or posting to the wrong issue. Outside our control.
> - **Network locality** — a remote Playwright reaches the *remote host's* localhost, not the developer's machine; testing a local dev server needs a port-forward (same trap noted for agentum's own browser pane).
> - **Scope size** — "ship all" makes this a large spec; risk of an oversized, hard-to-verify deliverable. *Mitigation:* Architect should slice into batches (precedent: spec 007).
>
> ---
>
> ## Notes
>
> **Out of scope (parked for a future round — none vetoed by the user):**
> - Authoring/generating the tests *for* the user (we run & drive existing tests + agent-driven checks; we don't write the suite from scratch).
> - Visual-regression / screenshot pixel-diffing.
> - Non-browser MCP servers (this is Playwright-MCP-specific).
> - External CI/CD pipeline integration (runs inside agentum sessions, not external CI).
> - Scheduling these checks on a cron (possible later Automation add-on).
>
> **Unifying frame.** One engine — "connect to the Playwright MCP browser and drive it on *this* host (local or remote)" — with two ways to invoke it ("run the suite" / "autonomous verify"). Per the interview these two collapse into one capability because, with MCP, both are simply browser work.
>
> **Concrete anchor.** v1 = reproduce the user's already-proven run — Ralph Loop + Playwright MCP, ~10 tasks, completed perfectly on localhost — but (a) launched from **Agents & Automation**, (b) on a **remote host**, and (c) reporting results to the **issue**.
>
> **Packaging.** Surfaced as an "agentic skill" / action on Agents & Automation; the actor is the agent itself running the loop. The developer's only action is *launch* + *read the issue comment*.
>
> **Scope caveat for the Architect.** The user explicitly chose "ship all" (engine + both invocation modes + local/remote parity + issue posting). This likely violates the "fits on one screen" guideline; recommend slicing into batches as spec 007 did rather than shrinking scope.
