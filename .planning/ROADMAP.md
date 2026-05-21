# Roadmap: Agentum — Kanban Orchestrator Milestone

## Overview

This milestone turns the existing kanban board into the orchestrator: a user drops a goal in, gets 3–7 linked cards out, claims one, and a tool session auto-spawns with the right context. Each phase is a thin vertical slice — schema change + backend route + watchdog wiring + dashboard + TUI surface — so every phase ends with something the user can demo end-to-end. v1 stops at "cards appear, ready to claim, and the binding works" — auto-claim chains are explicitly v2.

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: Goal → Cards (planner slice)** - User types a goal, a planner tmux session decomposes it into 3–7 linked child cards on the board.
- [ ] **Phase 2: Card ↔ Session binding** - Claiming a card auto-spawns the right tool with a context-loaded prompt; watchdog events stream back onto the card's comment thread.
- [ ] **Phase 3: Dependency gate + dispatch polish** - `blocked_by` edges enforce column transitions sub-10ms, opening prompts are previewable/editable, and agent activity surfaces inline as card comments.

## Phase Details

### Phase 1: Goal → Cards (planner slice)

**Goal**: User can submit a goal from the dashboard or TUI and watch a dedicated planner session populate the board with 3–7 linked child cards
**Mode:** mvp
**Depends on**: Nothing (first phase)
**Requirements**: SCHEMA-01, SCHEMA-02, SCHEMA-03, SCHEMA-04, SCHEMA-05, ORCH-01, ORCH-02, ORCH-03, ORCH-04, ORCH-05, ORCH-06
**Success Criteria** (what must be TRUE):

  1. User types a goal in the dashboard "Goal" composer (above the board) and within ~2 min sees 3–7 child cards appear in the `todo` column with the goal as their parent
  2. User submits a goal from the TUI via a new keybinding/overlay and the same cards appear on every connected client
  3. Each child card shows a visible parent-goal badge or grouping cue distinguishing it from a regular standalone card
  4. Cards persist across daemon restart with their `parent_goal_id` and `blocked_by` edges intact (existing pre-milestone cards still render unchanged)
  5. Planner tool + prompt are configurable per-server (under `.config/agentum/`) and fall back to `claude` when unset

**Plans**: 8 plans
Plans:
**Wave 1**

- [x] 01-01-PLAN.md — Schema + core types + Store CRUD (migration 0015 + BoardItem.parent_goal_id + Session.card_id + BoardLink + add_board_link/list_children_of_goal/max_child_status_rank/delete_board_link)
- [x] 01-02-PLAN.md — planner.toml config loader (planner_config_path + load_planner_config with bundled-prompt fallback + path-traversal guard)

**Wave 2** *(blocked on Wave 1 completion)*

- [ ] 01-03-PLAN.md — HTTP routes (POST /api/board/goals atomic create-goal + planner-spawn, POST/GET/DELETE /api/board/links with symbolic-key resolution, board.rs extended for parent_goal_id)

**Wave 3** *(blocked on Wave 2 completion)*

- [ ] 01-04-PLAN.md — Watchdog goal-status reconciler (subscribes to bus, recomputes max(child statuses), bypasses enforce_transition, planner auto-stop on first child)
- [ ] 01-05-PLAN.md — CLI shim (agentum board add-goal / add-card, credentials.toml-based auth, --key/--blocks symbolic resolution, no-token-in-argv)
- [ ] 01-06-PLAN.md — Dashboard surface (GoalComposer.svelte + submitGoal store action + parent-cue chip + filter pill + .lbl.goal CSS + SPA rebake)
- [ ] 01-07-PLAN.md — TUI surface (Overlay::Goal + G keybinding + Ctrl-Enter submit + parent-cue line + GOAL chip + o-to-jump-parent)

**Wave 4** *(blocked on Wave 3 completion)*

- [ ] 01-08-PLAN.md — End-to-end integration test + human-verify checkpoint (full happy-path through in-process daemon + ROADMAP success criteria 1-5 visual verification)

**UI hint**: yes

### Phase 2: Card ↔ Session binding

**Goal**: User can claim a card and have the right tool session auto-spawn, bidirectionally linked, with watchdog events surfacing as card comments
**Mode:** mvp
**Depends on**: Phase 1
**Requirements**: BIND-01, BIND-02, BIND-03, BIND-04, BIND-05, BIND-06
**Success Criteria** (what must be TRUE):

  1. User drags a `todo` card to `doing` (or clicks "Start") and a tool session spawns automatically with `card.session_id` and `session.card_id` both set in a single atomic write
  2. The card detail view shows a live tail snippet of the bound session's pane, a status pill, and an "open session" deep link; the session view reciprocally shows the bound card + parent goal with a "back to board" link
  3. Watchdog `AwaitingInput` / `AgentFinished` / `Crashed` events appear as `[system]` comments on the bound card in real time
  4. When a session crashes or is killed, the card stays in `doing` with a `[system]` crash comment (no auto-revert) — the user decides retry vs. move
  5. User can manually re-bind or unbind a card↔session pair from both dashboard and TUI; the binding survives daemon restart and profile switch

### Phase 3: Dependency gate + dispatch polish

**Goal**: User cannot move a card to `doing` while its `blocked_by` cards are unfinished, opening prompts can be reviewed/edited before dispatch, and the card stays the source of truth for agent activity
**Mode:** mvp
**Depends on**: Phase 2
**Requirements**: GATE-01, GATE-02, GATE-03, GATE-04, UX-01, UX-02, UX-03
**Success Criteria** (what must be TRUE):

  1. User attempting to move a card with an unfinished `blocked_by` edge to `doing` sees an inline "Blocked by #3: Set up DB" message (not a generic toast) in both dashboard and TUI; the PATCH returns HTTP 400 with `missing: ["dependency:<id>"]`
  2. Moving a card to `done` triggers a board event that lists the newly-unblocked dependents; connected clients refresh and reveal the now-claimable cards
  3. Auto-spawning a session shows the user a preview of the assembled opening prompt (card title + body + parent goal + summaries of `blocked_by` results); YOLO mode sends as-is by default, non-YOLO requires confirm
  4. The card's comment thread interleaves human comments with agent activity extracted from pane snapshots and watchdog events, so the user never has to retype "here's the plan, do step N"
  5. With 500 cards loaded on a board, every `enforce_transition` PATCH completes in <10ms (in-memory graph walk; benchmark recorded)

**UI hint**: yes

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Goal → Cards (planner slice) | 2/8 | In Progress|  |
| 2. Card ↔ Session binding | 0/TBD | Not started | - |
| 3. Dependency gate + dispatch polish | 0/TBD | Not started | - |
