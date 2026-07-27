---
schema: 1
id: SPC-0H1WSCFDNA8VJQMRZ2X6SQZE9D
revision: 1
title: Select which agent powers Chat + Create-issues
source: legacy-import:.agentum-harness/specs/394-create-issues-and-chat-in-the-config-we/spec.md@sha256:a2b3b7bff2bc9caa07aa890cdffbcbf04baad5e45b1edc902d506c988e8758d1
---

# Select which agent powers Chat + Create-issues

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

> # Spec 394 — Select which agent powers Chat + Create-issues
>
> > Generated from GitHub issue https://github.com/MateoCerquetella/agentum/issues/394
> > (verbatim title: "Create issues and chat in the. config we sohuld be able to select
> > with which AGENT to use" — no issue body). Refined by the PM gate.
>
> - **Number:** 394
> - **Status:** PM
> - **Surface:** `crates/agentum-server` (chat routes) + `crates/agentum-desktop/ui` (Chat page, Settings)
> - **Date:** 2026-07-20
>
> **User value (one line):** Chat and Create-issues work with the agent you already run — picked once in Settings — instead of requiring Claude.
>
> ## Problem
>
> The Chat intake interview and its Create-issues extraction only work with Claude. A user whose agent is Codex or Gemini hits a dead-end error on the very first message (`No LLM credentials for chat…`) and finds no setting to point the intake at the agent they already have installed — agentum's "any agent" promise (README: first-class Claude/Codex/Gemini/Hermes/OpenCode adapters) breaks exactly where feature planning starts.
>
> ## Goal
>
> Let the user select, once in Settings, which installed agent powers the Chat intake — the interview (`/api/chat/stream`) and its Create-issues extraction (`/api/chat/issues/preview`) are one screen sharing one backend (`routes/chat.rs`), so this is ONE slice, not two — defaulting to today's Claude behavior.
>
> ## Users / personas
>
> - **A developer whose daily agent is Codex (or Gemini)** — no Claude subscription, no `ANTHROPIC_API_KEY`. The moment: they open Mission Control → Chat to turn a feature idea into tracker issues, press send, and get `No LLM credentials for chat: set ANTHROPIC_API_KEY, or sign in to Claude…` (`NO_CREDS_MSG` in `crates/agentum-server/src/routes/chat.rs` — symbol cited, not line-pinned: the file is mid-edit on this branch). Settings offers nowhere to use the agent they already have.
>
> ## Acceptance criteria
>
> - [ ] Settings renders a **Chat agent** picker listing the installed agents reported by `/api/agents`, defaulting to Claude when installed (else the first installed agent, per the `pickDefaultAgent` precedent) so a Claude-less user never lands on a dead default.
> - [ ] The chosen chat agent persists in global settings across app restarts (same store precedent as `defaultTuiAgent`).
> - [ ] With a non-Claude agent selected (reference: Codex) and no Anthropic credentials present, sending a Chat message returns an assistant reply — today it returns the `NO_CREDS_MSG` error.
> - [ ] With a non-Claude agent selected (reference: Codex), **Preview issues** on a converged conversation returns the draft plan (title/summary/tasks) produced by that agent.
> - [ ] A selected agent that is not installed or not authenticated blocks the send and surfaces a typed error naming the agent and the fix — never a hang or an untyped 500.
> - [ ] With the setting untouched, `POST /api/chat/stream` returns replies via the existing Claude auth and the composer renders the same Claude-only model picker — zero behavior change for current users.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** one global chat-agent setting (store + Settings picker); the Chat interview and the `/api/chat/issues/preview` extraction honoring it, proven end-to-end with at least one non-Claude agent; a typed unavailable-agent error.
> - **Out:** per-conversation or per-mode (Fast/Socratic) agent overrides; per-agent model pickers (non-Claude agents run their own default model; the Claude model picker stays Claude-only); the harness `agent_tool` gated-run knob and the board-goals planner picker (already configurable — `harness/types.rs:115`, `board_goals.rs:745`); new agent adapters in `agentum-executor` (offer only what `/api/agents` detects); account/login management (AccountsPane's job); harness internals (spawn path, role gates, verdict-file contract) — untouched.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `/api/agents` installed-agent probe — `crates/agentum-server/src/routes/agents.rs:1,27`.
> - Agent catalog + default-pick helpers — `AGENT_CATALOG` (`crates/agentum-desktop/ui/src/lib/agent-catalog.tsx:23`) and `pickDefaultAgent` (`agent-catalog.tsx:456`).
> - Global agent-default setting precedent — `defaultTuiAgent` (`crates/agentum-desktop/ui/src/shared/types.ts:2090`); per-feature agent-selection Settings UI precedent — `components/settings/CommitMessageAiPane.tsx`.
> - Chat routes to EXTEND, not rewrite — `routes/chat.rs` `router()` registers `/api/chat`, `/api/chat/stream`, `/api/chat/issues/preview`, `/api/chat/issues`; the Claude path (`resolve_auth`, `chat_auth_gate`, `DEFAULT_MODEL`, `NO_CREDS_MSG` — symbols cited; line numbers drift while the branch WIP lands) stays as the default backend.
> - Claude-only chat model picker that STAYS Claude-only — `CHAT_MODELS` (`crates/agentum-desktop/ui/src/runtime/chat-client.ts:20-26`, all three entries Claude) rendered by `ModelPicker` in `components/harness/ChatPage.tsx`.
> - Non-Claude agent adapters — `crates/agentum-executor` (Claude/Codex/Gemini/Hermes/Opencode).
>
> ### Build new
>
> - The persisted chat-agent setting + Settings picker; a non-Anthropic chat backend selected by that setting (mechanism is the architect's call); the typed unavailable-agent error.
> - **Resume, don't restart.** This branch carries uncommitted scaffolding from the first execution pass (harness re-entered authoring per `decisions.md`): untracked `crates/agentum-server/src/routes/chat_agent.rs` (`ChatAgent` enum + request-/`chat.toml`-level agent resolution) and `routes/chat_openai.rs` (Codex Responses-API backend mirroring the Claude auth fallbacks), plus modified `chat.rs`, `routes/mod.rs`, `agentum-store/src/paths.rs`. The architect/developer assesses it for reuse or replacement — the acceptance criteria above are the bar either way.
>
> ## Adjacent, not duplicate
>
> - Spec 003 shipped the preview/draft flow this slice rides on; CHANGELOG 0.20.0 (#48) shipped an agent picker for the board-goals **planner** only — the conversational interview stayed Claude-only. Neither covers backend selection for Chat/Create-issues.
