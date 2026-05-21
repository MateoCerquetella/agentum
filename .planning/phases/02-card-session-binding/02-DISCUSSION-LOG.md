# Phase 2: Card ↔ Session binding - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-21
**Phase:** 02-card-session-binding
**Areas presented:** Auto-spawn trigger semantics, Watchdog → card-comment bridge, Re-bind / unbind UX, Card/session detail surfacing

---

## Pre-discussion analysis

Phase 1 had already locked the foundational shape (session.card_id, BoardItem.session_id, the events bus, the watchdog reconciler pattern, the spawn_planner_session helper, the 400 envelope from slice 1). The four gray areas presented below were the remaining HOW-to-implement questions, each with concrete options drawn from the existing codebase patterns rather than hypothetical alternatives.

## Area A — Auto-spawn trigger semantics

| Option | Description | Selected |
|--------|-------------|----------|
| Extend PATCH /api/board/{id} | Auto-spawn fires inside the existing patch handler when (status→doing AND session_id=null). Reuses enforce_transition gate site. | ✓ (Claude's discretion) |
| New POST /api/board/{id}/claim | Dedicated endpoint. Cleaner separation but grows the API surface. | |
| Two-step: explicit start | Force the client to PATCH status, then POST claim. Most pedantic, worst UX. | |

**Side-questions raised:**
- Missing-field policy when card.tool / card.workdir are NULL → resolved via fallback chain (card → parent_goal → daemon-wide default for tool; 400 for missing workdir)
- Initial prompt content → deferred (UX-01/UX-02 are Phase 3)

**User's choice:** "nothing lets execute i like the way u think" — Claude chose the PATCH-extension option (D-01..D-04).

## Area B — Watchdog → card-comment bridge

| Option | Description | Selected |
|--------|-------------|----------|
| New bus-subscriber task in agentum-watchdog | Sibling to run_goal_reconciler. Keeps the per-session watch_session loop focused. | ✓ (Claude's discretion) |
| Fold into watch_session per-session loop | Closest to the data, but couples pane classification with comment writes. | |
| Synthesize on read in the events route | No persistence; render-time only. Loses audit-trail value. | |

**Side-questions raised:**
- Comment shape → author="system", body templates with [system] prefix
- Dedupe → trust watchdog's existing activity-state transition gating, add an in-memory last-comment-kind map for defense-in-depth
- Goal-card filter → bridge SKIPS events where the bound card has lbl=goal

**User's choice:** Claude's discretion — selected the bus-subscriber pattern (D-05..D-09).

## Area C — Re-bind / unbind UX

| Option | Description | Selected |
|--------|-------------|----------|
| Extend existing PATCH with session_id double-Option | Same pattern Phase 1 used for parent_goal_id. No new endpoints. | ✓ (Claude's discretion) |
| Dedicated POST/DELETE /api/board/{id}/bind | Cleaner semantics, grows the surface. | |
| Two endpoints: /rebind and /unbind | Most explicit but pollutes the route table. | |

**Side-questions raised:**
- Previous-binding cleanup on rebind → atomic transfer in a single sqlx tx
- Auto-unbind on session crash → NO, per BIND-05 the binding stays so the user can navigate to the dead pane

**User's choice:** Claude's discretion — selected the PATCH-extension option (D-10..D-12).

## Area D — Card / session detail surfacing

| Option | Description | Selected |
|--------|-------------|----------|
| Polled GET /pane every 2 s, 20 lines, panel above comments in BoardItemDialog | Conservative cadence, reuses existing rate-limited capture-pane route. | ✓ (Claude's discretion) |
| Streaming WS /api/sessions/{id}/stream tail | Live but adds a new client-side subscription; complexity vs payoff TBD. | |
| Lazy-load on dialog expand | Lowest cost, loses the "live" feel. | |

**Side-questions raised:**
- Session view back-link shape → topbar chip with optional parent goal title
- TUI keybindings → `s` (card→session) and `c` (session→card), mnemonic-symmetric, single-keystroke

**User's choice:** Claude's discretion — selected polling (D-13..D-15) with a note that streaming WS may be swapped in at planning time if cheap.

## Claude's Discretion

The user stopped the interactive discussion before any area was walked through, with "nothing lets execute i like the way u think" — same shape as Phase 1's "no more questions, go code". All four areas (A, B, C, D) were resolved as Claude-discretion calls grounded in:

- Phase 1's CONTEXT.md decisions (D-01..D-14 there) for established patterns
- PROJECT.md constraints (no new SaaS dep, reuse over rebuild, embedded SPA rhythm)
- REQUIREMENTS.md BIND-01..BIND-06 (the locked contract)
- The codebase's existing route/store/watchdog/dashboard patterns

Decisions D-03 (no opening prompt in v1), D-05 (separate bus-subscriber vs folded), and D-13 (polling cadence) are explicitly flagged in CONTEXT.md as open for revision during planning if a tighter alternative surfaces.

## Folded Todos

- `.planning/todos/pending/2026-05-20-board-doing-create-test.md` — End-to-end test for the "doing" create path. Folded into Phase 2 scope; the auto-spawn IS the "create-into-doing via API" path the todo wants tested.

## Reviewed Todos (not folded)

- `.planning/todos/pending/board-transition-cas.md` — Compare-and-swap on board status PATCH. Reviewed and kept deferred. Belongs in Phase 3 alongside the dependency gate (same deferral Phase 1 captured).

## Deferred Ideas

Captured in CONTEXT.md's `<deferred>` block:

- Opening prompt assembly (UX-01, Phase 3)
- Preview/edit prompt before dispatch (UX-02, Phase 3)
- Dependency-aware column gate (GATE-01..04, Phase 3)
- Auto-claim chains, heartbeat reassignment, worktree fanout (v2)
- Auto-unbind on session crash (BIND-05 explicit; revisit if usage demands)
- Edit/delete board comments (future)
- Streaming WS for pane tail (revisit at planning time)
- Per-card pane-tail length config (hardcoded 20 in v1)
- CAS on board status PATCH (Phase 3 with the dependency gate)
- BoardComment.kind enum (deferred; author="system" suffices for v1)
