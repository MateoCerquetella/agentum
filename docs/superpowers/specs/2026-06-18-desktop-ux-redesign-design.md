# Desktop UX Redesign — Navigation + Spec→Kanban→Agent Pipeline

- **Date:** 2026-06-18
- **Status:** Design — awaiting review
- **Surface:** `crates/agentum-desktop/ui/` (React/Vite SPA)
- **Author:** Claude (brainstormed with Mateo)

## Problem

The desktop app's *engine* is good but its *navigation* makes it unusable:

1. **Hardlock.** Clicking the sidebar button labeled **"Agents"** switches to the
   `activity` view, which **hides the entire sidebar** (`App.tsx:1015`) and is the
   only top-level view with **no back button** (Tasks, Chat, Settings all have one).
   The only escape is a secret `Cmd+B`. Users get trapped.
2. **Labels lie.** The button labeled **"Agents"** opens an *activity feed*; the
   button labeled **"Chat"** opens a *feature-spec intake* (`harness` view). Nothing
   on screen explains what any view does.
3. **Buried concepts.** "Goals" lives inside Chat; a real Kanban board does not
   exist (the closest is the GitHub/Linear "Tasks" view).
4. **Net effect:** the user cannot form a mental model. "I don't understand anything."

## Decisions (from brainstorming)

| Question | Decision |
| --- | --- |
| Core loop | **Run & watch agents.** Everything else supports that. |
| Home screen | **Mission Control** — every agent across projects, grouped by status. |
| Navigation shape | **Persistent left rail** that never disappears + Mission Control home. Clicking an agent opens its workspace in the main pane; the rail stays. |
| Goals vs Board | **Board only.** A card *is* a goal/ticket. No separate "Goals" screen — fewest concepts to learn. |
| Rail order = workflow order | Home → **Chat/Spec (2nd)** → Board → Settings, mirroring the pipeline. |

## The pipeline (the spine of the app)

```
Spec (Chat)  ──►  Ticket(s)/cards  ──►  Board (Kanban)  ──►  Start card
                                                                  │
                                          agent creates a WORKTREE for the project
                                                                  │
                                                          agent runs in it
                                                                  │
                                                  Watch it LIVE in its workspace
```

Each rail item is the next step in that flow, top-to-bottom.

## Information architecture

### The left rail — always visible (kills the hardlock)

| Rail item | What it is | Replaces today's |
| --- | --- | --- |
| 🛰 **Mission Control** (Home) | Every agent you're running, grouped *Needs you / Working / Done*. Start new work from here. | "Agents" (the trap view) |
| 💬 **Chat / Spec** (2nd) | Describe what you want in plain words → produces ticket(s)/cards. | "Chat" (hidden spec intake) |
| ▦ **Board** (Kanban) | Cards = goals/tickets. `Backlog → Building → Review → Done`. Start a card → worktree + agent. GitHub/Linear issues flow in as cards. | new (subsumes "Tasks") |
| ⚙ **Settings** | Settings. | unchanged |

**Three rules that make it un-trappable:**
1. The rail **never disappears** — no full-page takeovers that hide navigation.
2. **Back + breadcrumb** on every drill-in screen.
3. **⌘K** from anywhere to jump to any agent, card, or screen.

Every screen carries a **title + one-line description** and **explained empty
states** (directly fixes "nothing is explained").

### Mission Control (Home)

```
┌──────┬─────────────────────────────────────────────┐
│🛰 Home│  Mission Control                  [ + New ]  │
│       │  Every agent you're running, grouped by      │
│💬 Chat│  what needs your attention.                  │
│       │  ─────────────────────────────────────────   │
│▦ Board│  ⏸ NEEDS YOU (1)                             │
│       │     payments-api · claude · "approve plan?"  │
│⚙ Set │  ● WORKING (3)                               │
│       │     auth-refactor · claude · editing files   │
│       │     fix-tests · codex · running tests        │
│       │  ✓ DONE (2)                                  │
│       │     logo-swap · gemini · ready to review     │
└──────┴─────────────────────────────────────────────┘
```

Click an agent → its workspace opens in the main pane; rail stays.

### Agent workspace (drill-in)

```
┌──────┬─────────────────────────────────────────────┐
│🛰 Home│ ← Mission Control   auth-refactor·claude·● │
│ …    │ ┌ Chat ─┬─ Code ─┬─ Card ┐                  │
│      │ │ live agent session (the terminal stream)  │
│      │ │ [ message the agent… ]               [↑]   │
└──────┴─┴────────────────────────────────────────────┘
```

- **← Back** + breadcrumb top-left, always.
- Three tabs: **Chat** (the live agent session — what the user calls "chatting
  with it"), **Code** (files / diff / browser preview), **Card** (the goal it's
  building + green/red verify status).

## What we reuse vs. build (grounded in code research)

### Already exists and is wired (do NOT rebuild)

- **Worktree creation** — `POST /api/worktrees/create` (`routes/worktrees.rs:269`,
  `git.rs:128`): runs `git worktree add`, persists registry, local + SSH.
- **One launch path** — `spawn_agent_into_pane()` (`routes/sessions.rs:539`):
  YOLO translation, loopback env, hooks, MCP wiring all centralized.
- **Session create with worktree isolation** — `POST /api/sessions` accepts a
  `worktree { branch, base_ref }` spec (`routes/sessions.rs:132`).
- **Start / reattach** — `POST /api/sessions/{id}/start` (`routes/sessions.rs:703`).
- **Live streaming** — `/api/sessions/{id}/stream` WS (`routes/sessions.rs:1006+`):
  pipe-pane log tail, snapshot/resume, resize/redraw heal.
- **Status events** — global bus + watchdog (`routes/events.rs`): `session.started`,
  `agent.awaiting_input`, `agent.finished`, `session.crashed`.
- **Board CRUD + status gates** — `/api/board` (`routes/board.rs`).
- **Card → agent auto-spawn** — PATCH card `todo→doing` already spawns an agent
  (`routes/board.rs:288` → `board_goals.rs:142` `spawn_card_session`).
- **Desktop streaming UI** — `components/Terminal.tsx` + `terminal-pane/TerminalPane.tsx`
  already render a live agent pane.

### Build new (mostly UI)

- **The left-rail nav shell** + Mission Control home + back/breadcrumb everywhere.
  Remove the sidebar-hiding logic (`App.tsx:1015`) and the label↔view scramble
  (`SidebarNav.tsx`, `store/slices/ui.ts:439`).
- **Board (Kanban) view** in the desktop UI (backend exists; no desktop UI today).
- **Agent workspace** drill-in (Chat/Code/Card tabs) reusing `Terminal.tsx` stream.
- **Spec → cards** UI flow (today harness features live in `.harness/feature_list.json`,
  separate from the board).

### The one real backend gap

`spawn_card_session` (`board_goals.rs:142`) currently spawns into `card.workdir`
**directly**, not a fresh worktree. To honor "the agent creates the worktree of that
project," the card-start path must first create a worktree (reuse
`/api/worktrees/create` / the session `worktree` spec) and spawn into it. Backend
primitives exist; only this orchestration is missing.

## Phased rollout (recommended)

**Phase 1 — Stop the bleeding (navigation shell).** Persistent left rail, Mission
Control as home (re-skin today's `activity` feed), back + breadcrumb + ⌘K
everywhere, plain labels + descriptions, kill the hidden-sidebar logic. *Almost
pure UI; immediate, dramatic usability win.* **Ship this first.**

**Phase 2 — The Board pipeline.** Desktop Kanban view; wire card "Start" →
create worktree → spawn agent (closes the one backend gap) → open the agent
workspace to watch live.

**Phase 3 — Spec → tickets + polish.** Chat/Spec front door produces cards;
agent-workspace Chat/Code/Card tabs; fold GitHub/Linear "Tasks" into the board as
a sync source.

## Non-goals (YAGNI)

- No new agent adapters, no backend rewrite — reuse the existing launch path.
- No multi-board / swimlane complexity in v1 — four columns, one board.
- Keep the existing terminal/editor/browser workspace; re-home it as the agent
  drill-in, don't replace it.

## Open questions

- Board columns: confirm `Backlog → Building → Review → Done` maps cleanly onto the
  existing `todo / doing / done` status model (we'd add a "review" status).
- Does "Start a card" create a worktree per card, or per project (reused across
  that project's cards)? Leaning **per card** (isolation), prune on card done.
```
