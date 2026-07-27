---
schema: 1
id: SPC-0ERJR457AJZCEWVR88Z4N71VMS
revision: 1
title: SDD toolbar session fidelity across agents
source: legacy-import:ai/specs/024-sdd-toolbar-session-fidelity/spec.md@sha256:fc5f8fc7c0ddf9ace209631a84e7f670d884561f4cb1e90fa23112932955e9b6
---

# SDD toolbar session fidelity across agents

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

> # Spec 024 — SDD toolbar session fidelity across agents
>
> - **Number:** 024
> - **Status:** Done
> - **Surface:** `crates/agentum-desktop/ui` + `crates/agentum-server/src/routes/sdd.rs`
> - **Author:** Mateo Cerquetella
> - **Date:** 2026-07-21
> - **Tracker:** https://github.com/MateoCerquetella/agentum/issues/412
>
> ## Problem
>
> The SDD toolbar is unreliable on agent terminals: it sometimes disappears,
> an action may reach the wrong or stale session, and a tab opened as Codex may
> present Claude Code instead. This makes it unsafe to continue an SDD workflow
> without first checking the pane manually.
>
> ## Goal
>
> Let a user operate the SDD toolbar on any supported agent terminal with every
> control remaining bound to the agent session visible in that pane.
>
> ## Users / personas
>
> - **Mateo, an Agentum dogfooder**, uses Claude, Codex, and other agent terminals
>   side by side and needs each terminal’s SDD controls to stay visible and act on
>   only that terminal.
>
> ## User value
>
> Continue an SDD workflow from the visible terminal without checking whether the
> toolbar disappeared, the agent changed, or the prompt went to another pane.
>
> ## Acceptance criteria
>
> - [x] The expanded toolbar renders no standalone “SDD” label; `Spec`, `Spec
>   Socratic`, and `Status` stay on the left, while `Continue` renders immediately
>   before `Loop` on the right.
> - [x] A running agent session keeps its SDD toolbar mounted through idle titles,
>   reconnects, missing hook updates, and temporary process-detection failures; a
>   true plain-shell session renders no actionable SDD toolbar, and the explicit
>   Hide/Show preference still works.
> - [x] Every toolbar action resolves the server session bound to the visible pane
>   and uses that same session for injection, loop reads/toggles, and event
>   filtering; switching tabs or split groups cannot target the previous pane.
> - [x] Choosing any desktop-supported agent preserves that identity through
>   launch, attach, restore, and SDD delivery. In particular, choosing Codex
>   renders and controls Codex, never Claude; an incompatible existing session is
>   rejected visibly rather than attached as the requested agent.
> - [x] `Spec`, `Spec Socratic`, `Continue`, `Status`, and Loop steps inject through
>   the existing server path for every recognized agent: MCP-provisioned agents
>   receive the bootstrap and other agents receive the full playbook; a plain
>   shell never receives playbook prose as a command.
> - [x] The toolbar renders “sent” only after delivery to the targeted pane is
>   confirmed, and renders an actionable failure for a missing/stopped session,
>   tool mismatch, unknown playbook, or injection failure.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** toolbar arrangement; stable pane/session/agent identity; fail-closed
>   agent attachment; truthful cross-agent injection outcomes.
> - **Out:** changing playbook text, loop limits, settle timing, completion rules,
>   stop reasons, Harness Engine gates, or the global usage/status bar; adding a
>   new agent; enabling SDD actions in a true shell.
> - This does not redesign issue #395’s read-only gated-run phase strip or spec
>   399’s broader gated-workspace progress surface.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `SddBarGate` / `SddBar` (`crates/agentum-desktop/ui/src/components/sdd/SddBar.tsx:156`)
>   — keep the one toolbar, preview, collapse preference, and server-event model.
> - `TabGroupPanel` (`components/tab-group/TabGroupPanel.tsx:381`) — keep the real
>   split-group mount point below each active terminal pane.
> - `server:<sessionId>:<leafId>` registration
>   (`components/terminal-pane/server-pane-connection.ts:381`) — keep this as the
>   pane-to-session address.
> - `launchAgentInNewTab` (`ui/src/lib/launch-agent-in-new-tab.ts:205`) and
>   `ensureWorkspaceSession` (`ui/src/runtime/workspace-session.ts:100`) — retain
>   the single launch and host-aware per-tab attach paths.
> - `/api/sessions/{id}/sdd/inject` and `prompt_for`
>   (`crates/agentum-server/src/routes/sdd.rs:105`, `:159`) — retain server-owned
>   playbooks, host-aware delivery, and MCP/full-playbook selection.
> - `spawn_agent_into_pane` and `agentum_executor::adapter_for` remain the only
>   server launch path and adapter registry.
>
> ### Build new
>
> - One stable toolbar identity projection from pane binding, server session tool,
>   requested launch identity, and live agent evidence.
> - A fail-closed requested-tool/session-tool compatibility check, including
>   reconciliation when a recognized agent was started manually in a shell.
> - A delivery-result contract so queued, delivered, and failed injections are
>   distinguishable in the toolbar.
>
> ## Risks & invariants
>
> - A pane’s bound session ID—not workdir, title, or global active tab—is the only
>   valid injection address.
> - Every spawn remains on `spawn_agent_into_pane`; never type a second launch
>   command into a reattached agent.
> - Preserve MCP-first delivery, the full-playbook fallback, remote-host routing,
>   and `inject_prompt` readiness plus two-step submit mechanics.
> - Requested agent is intent; the bound server session is persisted identity;
>   live evidence may reconcile a shell but may not silently relabel one agent as
>   another.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** `terminal-sdd-toolbar-session-fidelity`.
> - **`verify.sh` asserts:** focused Vitest for layout, stable visibility,
>   split-pane isolation, and Codex/Claude identity; Rust delivery-result and
>   MCP/full-agent-matrix tests; UI build, server lib tests, and fmt check.
> - **`qa.sh` asserts:** open Claude and Codex in separate splits, exercise an SDD
>   action in each through idle/reconnect, verify no cross-pane delivery or agent
>   substitution, and verify full-playbook delivery on one non-MCP agent; capture
>   the final toolbar layout.
>
> ## Open questions
>
> - None. The architect may choose synchronous delivery confirmation or a
>   correlated result event, provided the final toolbar outcome is truthful.
