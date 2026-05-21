# Requirements: Agentum — Kanban Orchestrator Milestone

**Defined:** 2026-05-20
**Core Value:** One terminal — and one orchestrator — to manage all your AI coding agents across all your projects. The kanban *is* the orchestrator: a goal in, executing cards out.

## v1 Requirements

Requirements for this milestone. Each maps to one roadmap phase.

### Schema (data model extensions)

- [ ] **SCHEMA-01**: `BoardItem` gains `parent_goal_id: Option<i64>` so child cards point at the goal that spawned them
- [ ] **SCHEMA-02**: New `board_links` table stores `(from_card_id, to_card_id, kind)` with `kind ∈ {blocks, parent_of}`
- [ ] **SCHEMA-03**: Numbered migration (0015+) creates the new column + table; default behavior unchanged for cards without parents/edges
- [ ] **SCHEMA-04**: `Session` gains `card_id: Option<i64>` so the daemon can resolve session↔card both ways without scanning
- [ ] **SCHEMA-05**: `agentum-core` types (`BoardItem`, `Session`, `BoardLink`) updated and serde-compatible with existing API consumers

### Orchestrator (goal → cards)

- [ ] **ORCH-01**: User can submit a goal from the dashboard (new "Goal" composer above the board)
- [ ] **ORCH-02**: User can submit a goal from the TUI (new keybinding / overlay)
- [ ] **ORCH-03**: Submitting a goal spawns a dedicated planner tmux session running the user's configured planner tool, with an orchestrator prompt that emits cards via `/api/board`
- [ ] **ORCH-04**: Planner session writes 3–7 child cards within ~2 minutes; each card has `parent_goal_id` set and `blocked_by` edges where appropriate
- [ ] **ORCH-05**: Planner tool + prompt are configurable per-server (config in `.config/agentum/`); falls back to `claude` if unset
- [ ] **ORCH-06**: Dashboard and TUI display goals distinctly from regular cards (badge / filter / column hint)

### Binding (card ↔ session, fix the broken feature)

- [ ] **BIND-01**: PATCHing a card to `status=doing` with no `session_id` auto-spawns the matching tool session, sets `card.session_id` + `session.card_id` atomically, and returns the updated card
- [ ] **BIND-02**: Card detail view shows the bound session (live pane tail snippet, status pill, "open session" link)
- [ ] **BIND-03**: Session view shows the bound card and its parent goal, with a deep link back to the board
- [ ] **BIND-04**: Watchdog `AwaitingInput` / `AgentFinished` / `Crashed` events post automatic system comments to the bound card's comment thread
- [ ] **BIND-05**: Session crash or kill leaves the card in `doing` with a `[system]` comment; status does NOT auto-revert (user decides whether to retry or move)
- [ ] **BIND-06**: User can manually re-bind / unbind a card↔session pair via the dashboard and TUI; binding survives daemon restart and profile switch

### Gate (dependency-aware column rules)

- [ ] **GATE-01**: `enforce_transition` rejects `status=doing` when any `blocked_by` edge points to a card not in `done`, returning HTTP 400 with `missing: ["dependency:<id>"]`
- [ ] **GATE-02**: Dashboard and TUI surface the 400 inline ("Blocked by #3: Set up DB"), not as a generic toast
- [ ] **GATE-03**: Moving a card to `done` emits a board event listing newly-unblocked dependents; clients refresh
- [ ] **GATE-04**: Graph walk for the gate stays sub-10ms on 500-card boards (in-memory traversal, no per-edge SQL)

### Dispatch UX (kill the prompt-rewrite tax)

- [ ] **UX-01**: Auto-spawned sessions get an opening prompt assembled from card title + body + parent goal summary + summaries of `blocked_by` cards' results
- [ ] **UX-02**: User can preview / edit the opening prompt before dispatch (YOLO mode honors a "send as-is" default; non-YOLO requires confirm)
- [ ] **UX-03**: Card comment thread displays agent activity (extracted from pane snapshots and watchdog events) inline alongside human comments, so the card stays the source of truth without retyping context

## v2 Requirements

Deferred — acknowledged but not in this milestone's roadmap.

### Autonomous execution

- **AUTO-01**: First-unblocked card auto-claims a session when its dependencies hit `done`
- **AUTO-02**: Full-graph hands-off execution: orchestrator + auto-claim chained until all leaves are `done` or `blocked`
- **AUTO-03**: Heartbeat-driven reassignment: if a session goes silent past threshold, reassign the card to a new session

### Worktree fanout

- **WT-01**: Parallel cards execute in isolated git worktrees so they don't clobber each other
- **WT-02**: Worktree cleanup on card done / archived

### Personas

- **PERSONA-01**: Per-tool skill profiles (Hermes-style) layered on top of the existing tool-axis selection

## Out of Scope

Explicitly excluded from this milestone. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| In-process LLM call for orchestration | Locked decision in PROJECT.md — planner is a tmux session; keeps the orchestrator on the same code path as every other agent |
| N:N card↔session binding | Locked decision in PROJECT.md — 1:1 only; spike-style multi-session work belongs in a separate workflow |
| Parallel "task" entity alongside `BoardItem` | Locked decision in PROJECT.md — extend the existing schema; second concept causes divergence and double-write bugs |
| Multi-agent personas / skill profiles | v2 — current tool-axis selection (claude / codex / …) is sufficient for now |
| Telegram / external push notifications | Out — dashboard + TUI are the two surfaces; adding a third multiplies UX work |
| Hands-off full-graph execution | v2 — milestone stops at "cards appear, ready to claim" by design; chained execution is a separate quality bar |
| Backwards-incompatible board schema change | Constraint — existing cards (no parent_goal_id, no edges) must keep working; new columns nullable |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| SCHEMA-01 | Phase 1 | Pending |
| SCHEMA-02 | Phase 1 | Pending |
| SCHEMA-03 | Phase 1 | Pending |
| SCHEMA-04 | Phase 1 | Pending |
| SCHEMA-05 | Phase 1 | Pending |
| ORCH-01 | Phase 1 | Pending |
| ORCH-02 | Phase 1 | Pending |
| ORCH-03 | Phase 1 | Pending |
| ORCH-04 | Phase 1 | Pending |
| ORCH-05 | Phase 1 | Pending |
| ORCH-06 | Phase 1 | Pending |
| BIND-01 | Phase 2 | Pending |
| BIND-02 | Phase 2 | Pending |
| BIND-03 | Phase 2 | Pending |
| BIND-04 | Phase 2 | Pending |
| BIND-05 | Phase 2 | Pending |
| BIND-06 | Phase 2 | Pending |
| GATE-01 | Phase 3 | Pending |
| GATE-02 | Phase 3 | Pending |
| GATE-03 | Phase 3 | Pending |
| GATE-04 | Phase 3 | Pending |
| UX-01 | Phase 3 | Pending |
| UX-02 | Phase 3 | Pending |
| UX-03 | Phase 3 | Pending |

**Coverage:**
- v1 requirements: 24 total
- Mapped to phases: 24 (100%)
- Unmapped: 0

---
*Requirements defined: 2026-05-20*
*Last updated: 2026-05-20 after roadmap creation*
