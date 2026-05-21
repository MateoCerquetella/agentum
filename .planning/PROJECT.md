# Agentum

## What This Is

Agentum is a self-hosted control plane for orchestrating multiple AI coding agents (Claude Code, Codex, OpenCode, Cursor, Gemini, Hermes, …) from a single terminal-or-browser interface. A local daemon owns a tmux server where each session is one agent CLI; a SvelteKit dashboard and a Rust TUI drive it via an HTTP/WS API. It's for solo developers and small teams who want AI agents that keep running on their own hardware while they're away from their desk — including from a phone.

## Core Value

> One terminal — and one orchestrator — to manage all your AI coding agents across all your projects, even when your laptop is closed. The kanban *is* the orchestrator: a goal in, executing cards out.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. Inferred from the codebase + PRD.md + recent releases (v0.8.x). -->

- ✓ Self-hosted daemon (`agentum serve`) with embedded dashboard SPA — shipped
- ✓ TUI (`agentum terminal`) over the same HTTP/WS API as the dashboard — shipped
- ✓ Multi-vendor tool adapters with installation gating: Claude Code, Codex, Cursor, Gemini, Hermes, OpenCode, Aider, plus terminal/bash passthrough — shipped
- ✓ YOLO marker translation: clients push the canonical Claude marker, each adapter translates to its tool's spelling — shipped
- ✓ Tmux-backed session lifecycle (new/send-keys/capture-pane/kill) — shipped
- ✓ Watchdog: tails panes, emits AgentFinished / AwaitingInput / Crashed events — shipped
- ✓ SQLite persistence (14 migrations: sessions, board, notes, channels, users, auth, column rules) — shipped
- ✓ Multi-endpoint connection profiles (CLI `profiles add/list/rm/use`, TUI `Ctrl-S` overlay, dashboard endpoint switcher) — shipped
- ✓ TLS + token auth (TOFU bootstrap via `/api/cert/fingerprint`) — shipped
- ✓ Kanban board with claim/release/comments/reorder + per-server column rules (slice 1 const matrix + slice 2 DB overrides) — shipped through v0.8.2

### Active

<!-- Current milestone: Hermes-style runtime orchestration on top of the existing kanban. -->

- [ ] **Goal → task triage:** drop a single goal into the board, a dedicated planner session (an existing agent CLI running an orchestrator prompt) decomposes it into 3–7 linked kanban cards with titles, bodies, and `blocks` / `blocked_by` edges
- [ ] **Card↔session binding (fix the broken feature):** a card holds a `session_id` and a session knows its `card_id`; the link survives daemon restarts, dashboard reloads, and TUI profile switches; comments stream both directions
- [ ] **1:1 auto-spawn on claim:** moving a card to `doing` (or clicking "Start") spawns the right tool session, injects the card title/body/parent-context as the first prompt, and routes watchdog events back onto the card's comment stream
- [ ] **Dependency-aware doing-column gate:** can't move a card to `doing` while its `blocked_by` cards are not `done` — wired through the existing `enforce_transition` path so the dashboard and TUI both surface the 400
- [ ] **Reduce prompt rewrite tax:** the card body + parent goal + linked-card summaries are the only context the agent needs to start work on a card (no more pasting "here's the plan, do step N" by hand)

### Out of Scope

<!-- Boundaries we will not cross in this milestone, with reasoning so we don't relitigate. -->

- **Multi-agent personas / skill profiles** — Hermes assigns skill profiles per agent. Agentum's selection is by `tool` (claude/codex/…). Adding personas is a separate axis and would expand the data model significantly. Revisit only if tool-axis selection proves insufficient.
- **Telegram / external push notifications** — the dashboard + TUI are the interface; adding a third surface multiplies UX work. Revisit if mobile-without-PWA becomes a real ask.
- **In-process LLM call for orchestration** — locked decision: the planner is a tmux session, not a daemon-side API client. Keeps the orchestrator on the same code path as every other agent and avoids a new external API-key dependency.
- **N:N card↔session** — locked decision: 1:1 binding. Multi-session spike work belongs in a separate spike workflow, not the main board.
- **Hands-off full-graph execution** — v1 stops at "cards appear, ready to claim." Auto-claim chains, heartbeat-driven reassignment, and worktree fanout are explicitly v2+.
- **Parallel "task" entity alongside `BoardItem`** — locked decision: extend the existing schema, don't introduce a second concept. Risks otherwise: divergence, double-write bugs, two UIs.

## Context

- **Existing surface:** dashboard (SvelteKit, embedded via `rust-embed`) + TUI (`crates/agentum/src/commands/terminal/`) + HTTP/WS API (`crates/agentum-server/src/routes/`). Kanban routes already exist in `board.rs` and `board_rules.rs`.
- **Existing board model:** `BoardItem { id, title, lbl, workdir, tool, status, priority, claimed_by, session_id, … }` lives in `crates/agentum-core/src/lib.rs`. Comments via `BoardComment` + `/api/board/{id}/comments`. The `session_id` field already exists but the binding has gaps — that's the "broken feature" this milestone fixes.
- **In-flight specs:** `.planning/specs/2026-05-19-typed-kanban-card-schemas.md` (shipped, slice 1 const matrix) and `.planning/specs/2026-05-20-board-column-rules-overrides.md` (in-progress, slice 2 DB overrides). The orchestrator milestone is slice 3+ of this same thread — natural continuation.
- **What "Hermes-style" means here:** Nous Research's [Hermes Agent Kanban](https://hermes-agent.nousresearch.com/docs/user-guide/features/kanban) — orchestrator decomposes a goal into a linked kanban graph, agents read/write cards mid-turn (list/show/context/tail/watch/comment/block/unblock/assign/link), SQLite-backed, live dashboard. We are adopting the *shape*, not the codebase.
- **Why this matters right now:** the user reports having to retype the same orchestration prompts ("here's the plan, do step N") into agent sessions repeatedly. The kanban is supposed to carry that context but the card↔session link is incomplete, so cards stay decorative instead of operational.
- **User profile:** Mateo (solo, Rust + Svelte fluent, deep on the agentum codebase). Working from existing PRD.md and CLAUDE.md as authoritative product/architecture references.

## Constraints

- **Tech stack:** Rust 1.85 / edition 2024 workspace + SvelteKit dashboard embedded via `rust-embed`. Adding the orchestrator must not introduce a non-Rust daemon dependency or break the single-binary distribution.
- **Reuse over rebuild:** every new endpoint extends `crates/agentum-server/src/routes/` with the same auth middleware + `AppState` shape. Every new schema column lives behind a numbered migration in `crates/agentum-store/migrations/` (next: `0015_*.sql`).
- **No new SaaS dependency:** the orchestrator must use whatever agent CLI the user already has installed (probed via `/api/agents`). No daemon-side Anthropic/OpenAI API key.
- **Backwards compatibility:** existing board cards (no `parent_goal_id`, no dependency edges) must keep working unchanged. Migrations add columns nullable; default `enforce_transition` behavior is preserved.
- **Embedded SPA rebuild rhythm:** dashboard changes require `npm run build --prefix dashboard && cargo build --release` to bake into the binary. Plans must account for this; CI must catch a missed rebuild.
- **Performance:** the dependency-aware column gate runs on every PATCH. Must stay sub-10ms even with hundreds of cards (in-memory graph walk; no per-edge SQL query).

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Orchestrator runs as a tmux session (not an in-process LLM call) | Reuses the existing session pipeline, watchdog, transcript handling; no new API-key surface; planner is just another agent the user can swap | — Pending |
| Card↔session is 1:1 | Simplest mental model; matches how the user already works; multi-session spike workflows belong elsewhere | — Pending |
| Reuse `BoardItem` — add columns instead of a parallel `task` entity | One UI, one source of truth, no double-write bugs; dependency edges land as a separate `board_links` table referencing `BoardItem.id` | — Pending |
| No external SaaS dep for orchestration | Distribution stays a single self-hosted binary; user controls their own model choice via existing adapters | — Pending |
| V1 stops at "cards appear, ready to claim"; auto-claim chains are v2 | De-risks the milestone; the planner-decomposition and binding-fix are the leveraged pieces; chained execution is a separate quality bar | — Pending |
| Adopt Hermes' *shape* (goal→graph→cards), not Hermes' codebase | Hermes is a different product (Nous Research, separate stack); we want the UX pattern and the orchestration model, integrated with agentum's existing primitives | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-05-20 after initialization (orchestrator milestone)*
