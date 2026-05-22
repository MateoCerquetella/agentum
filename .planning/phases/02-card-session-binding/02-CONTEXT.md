# Phase 2: Card ↔ Session binding - Context

**Gathered:** 2026-05-21
**Status:** Ready for planning

<domain>
## Phase Boundary

When a user moves a card from `todo → doing` (drag in the dashboard, PATCH
from the TUI/CLI, or "Start" button), the daemon atomically spawns a tool
session bound to that card and writes both halves of the link
(`card.session_id`, `session.card_id`). The watchdog's existing
`agent.awaiting_input` / `agent.finished` / `session.crashed` events are
bridged onto the bound card's comment thread as `[system]` rows so the card
becomes the durable record of agent activity. The card detail view shows a
live pane tail and an "open session" deep link; the session view shows a
back-link to the card (and its parent goal, if any). Crashes leave the card
in `doing` with a `[system]` crash comment — no auto-revert. Users can
manually re-bind or unbind a card↔session pair via PATCH from either
surface; the binding survives daemon restart and profile switch.

**In scope:**
- PATCH-triggered auto-spawn on `status → doing` when `card.session_id IS
  NULL` (BIND-01); atomic dual-write via a new `Store::claim_card` method
- Missing-field policy for spawn: NULL `tool` → daemon-wide default
  (`claude`); NULL `workdir` → inherit from `parent_goal.workdir`; both NULL
  → HTTP 400 with the existing `{ missing, status }` envelope from slice 1
- Watchdog→comment bridge: a new bus-subscriber task in `agentum-watchdog`
  that filters bus events to `agent.awaiting_input` / `agent.finished` /
  `session.crashed` for sessions where `card_id IS NOT NULL` and inserts
  a `board_comments` row with `author="system"` (BIND-04)
- Card detail "Bound session" panel above comments in
  `BoardItemDialog.svelte`: status pill, 20-line pane tail (polled
  `GET /api/sessions/{id}/pane` every 2 s while the dialog is open), and
  "Open session →" button (BIND-02)
- Session view back-link chip in the topbar of `/sessions/[id]` that
  reads `← Card #N` (with parent goal title appended if present) and
  navigates to the board view (BIND-03)
- TUI parity: in the Board view, key `s` on a card with `session_id` set
  jumps to that session pane; in the Session view, key `c` jumps back to
  the bound card. Both follow the existing single-letter overlay pattern
- Crash behavior: card stays in `doing`; `session.card_id` and
  `card.session_id` remain set; a single `[system] session crashed: …`
  comment is posted (BIND-05). User can manually retry/unbind
- Manual re-bind/unbind through `PATCH /api/board/{id}` accepting
  `session_id: null` (explicit unbind) or `session_id: <uuid>` (explicit
  rebind); double-Option pattern that 01-01 established for
  `parent_goal_id`. Atomic transfer clears the previous session's
  `card_id` in the same transaction (BIND-06)

**Out of scope (explicitly deferred):**
- Opening prompt assembly from card title + body + parent goal + sibling
  summaries → Phase 3 (UX-01)
- Preview/edit of the opening prompt before dispatch → Phase 3 (UX-02)
- Dependency-aware column gate (`blocked_by` enforcement) → Phase 3
  (GATE-01..04). Phase 2 leaves the `enforce_transition` body unchanged
- Auto-claim chains / first-unblocked-card auto-spawn → v2 (AUTO-01..03)
- Worktree fanout per card → v2 (WT-01..02)
- N:N card↔session binding (locked decision)
- Edit/delete board comments (existing `BoardComment` doc-comment defers
  this — keep deferred)
- Per-card configurable pane-tail length, dashboard setting, streaming-WS
  pane subscription. Fixed 20 lines on a 2 s poll in v1
- Auto-unbinding a dead session on crash (BIND-05 says user decides;
  keeping the link visible is the simplest read)

</domain>

<decisions>
## Implementation Decisions

The user stopped the interactive discussion before any area was walked
through, saying "nothing lets execute i like the way u think" (same
shape as Phase 1's "no more questions, go code"). The decisions below
are Claude's best inference from Phase 1's CONTEXT.md, PROJECT.md
constraints, REQUIREMENTS.md, and the existing codebase. They are open
to revision at plan-review or during execution if the planner / executor
surfaces a better fit.

### Area A — Auto-spawn trigger semantics

- **D-01:** Trigger lives inside the existing `patch` handler in
  `crates/agentum-server/src/routes/board.rs` (around `:191`). When the
  request resolves to `target_status = "doing"` AND the merged
  `session_id` is `None`, the handler calls a new
  `Store::claim_card(card_id, …) -> (BoardItem, Session)` method that
  runs in a single sqlx transaction:
    1. Resolve `tool` / `workdir` per **D-02**
    2. INSERT a new `sessions` row with `card_id = card.id`
    3. UPDATE `board_items` set `session_id = session.id` for the card
    4. Return both updated rows
  Same dual-write contract that Phase 1's `POST /api/board/goals` uses
  for the planner spawn (`spawn_planner_session` helper in
  `board_goals.rs`), reframed as a PATCH side-effect. Rationale: BIND-01
  says "PATCHing a card to status=doing with no session_id"
  word-for-word — keeping the trigger inside the existing PATCH handler
  matches that wording, avoids growing the API surface, and reuses the
  existing `enforce_transition` gate site.
- **D-02:** Missing-field policy for the spawn. The card may have NULL
  `tool` and/or NULL `workdir` (Phase 1's planner-emitted children don't
  always carry them). Resolution order:
    - `tool`: `card.tool` → `parent_goal.tool` → `"claude"` (same daemon-
      wide default the planner uses; see Phase 1 D-14)
    - `workdir`: `card.workdir` → `parent_goal.workdir` → 400
  If `workdir` cannot be resolved, return HTTP 400 with the existing
  envelope `ApiError::Custom(BAD_REQUEST, json!({ "missing":
  ["workdir"], "status": "doing" }))`. Matches slice 1's
  `enforce_transition` 400 shape exactly so dashboard + TUI surfacing
  pipelines (Phase 1 of slice 1) work unchanged.
- **D-03:** **No opening prompt content in Phase 2.** The session
  spawns blank; the user's first manual message is whatever they type
  into the pane. UX-01 (prompt assembly) and UX-02 (preview/edit) are
  Phase 3. Keeps the v1 "ready to claim" envelope intact and avoids
  re-engineering the prompt path twice.
- **D-04:** The PATCH response shape on auto-spawn is the existing
  `BoardItem` JSON (the patched card) — same as today's PATCH. The
  newly-created session is NOT inlined into the response; clients fetch
  it via `GET /api/sessions/{card.session_id}` when they need it, and
  the existing `board.updated` event already carries the new
  `session_id`. Rationale: keeping the response shape stable avoids
  forcing TypeScript clients to re-derive the BoardItem-vs-{board,
  session} discriminator.

### Area B — Watchdog → comment bridge

- **D-05:** Bridge lives in a **new bus-subscriber task** in
  `agentum-watchdog/src/lib.rs`, sibling to Phase 1's
  `run_goal_reconciler`. Name: `run_session_comment_bridge`. Wired
  from `agentum-server/src/lib.rs` in the same place the reconciler is
  spawned. Rationale: keeps the per-session `watch_session` loop focused
  on its pane-classifier job; the reconciler pattern is already
  validated, tested, and documented from Phase 1 plan 01-04.
- **D-06:** **Comment shape.** `author = "system"` (free-form string,
  already used by Phase 1's planner events as a convention). Body
  templates (single-line, lowercase, `[system]`-prefixed for
  visual scanning):
    - `agent.awaiting_input` → `[system] agent awaiting input`
    - `agent.finished`       → `[system] agent finished`
    - `session.crashed`      → `[system] session crashed: <signature>`
      (substitute `Event.payload.signature` when present; literal
      `unknown` when not). Trim signature to 80 chars max
  Comments are render-only (existing `BoardComment` doc-comment), so the
  schema needs no new column.
- **D-07:** **Dedupe = trust the watchdog.** Phase 1's classifier
  already emits agent.* events only on activity-state transitions (see
  `crates/agentum-watchdog/src/lib.rs:386-432`). The bridge inserts one
  comment per event with NO additional dedupe layer. Idempotency hedge:
  the bridge stores the last comment's `(session_id, kind)` in an
  in-memory `HashMap<Uuid, &'static str>` and skips identical
  back-to-back inserts (defense-in-depth against bus-lag double-fires —
  same pattern Phase 1's `planner_stopped: HashSet<i64>` used for the
  auto-stop idempotency).
- **D-08:** **Goal-card filter.** The bridge SKIPS events where the
  bound card has `lbl = "goal"` — planner sessions bind to goal cards,
  and we don't want `[system] agent finished` cluttering the goal
  thread (the goal-status reconciler already surfaces "finished" via
  the `goal.status.changed` flow). Bridge filter: `session.card_id IS
  NOT NULL AND board_items.lbl IS DISTINCT FROM 'goal'`.
- **D-09:** **Bus lag policy.** On `RecvError::Lagged(n)`, the bridge
  logs `tracing::warn!(skipped = n, "session_comment_bridge: bus
  lagged")` and continues. No re-sync (unlike the goal reconciler,
  which recomputes `max_child_status_rank` on every event so it
  self-heals). Rationale: a missed `[system]` comment is benign — the
  card thread is informative, not a source of truth for state. The
  state is held by `session.status` and `card.status`, which the
  reconciler self-heals.

### Area C — Re-bind / unbind UX

- **D-10:** **HTTP shape: extend existing PATCH.** Add `session_id` to
  `BoardPatch` with the same double-Option pattern 01-01 used for
  `parent_goal_id` (`deserialize_optional_field` → `Option<Option<…>>`),
  so the body can distinguish "field omitted" (keep current value),
  "field = null" (explicit unbind), and "field = <uuid>" (explicit
  rebind). NO new endpoints, NO `/bind` or `/unbind` route. Rationale:
  the patch handler is already the canonical mutation site, the
  pattern is tested in Phase 1, and dashboard's existing PATCH client
  doesn't need a new affordance.
- **D-11:** **Atomic transfer on rebind.** A new
  `Store::transfer_card_binding(card_id, new_session_id) -> Result<()>`
  method runs in a single sqlx transaction:
    1. SELECT current `card.session_id` (old binding, may be NULL)
    2. UPDATE `sessions` SET `card_id = NULL` WHERE `id = old_session_id`
       (no-op when NULL)
    3. UPDATE `sessions` SET `card_id = card_id` WHERE `id =
       new_session_id` (errors if the new session is already bound to a
       different card → returns `StoreError::Conflict`)
    4. UPDATE `board_items` SET `session_id = new_session_id` WHERE
       `id = card_id`
  Unbind (new_session_id = NULL): skip step 3, set the card column to
  NULL in step 4. Conflict on step 3 surfaces as HTTP 409.
- **D-12:** **Crash leaves binding intact.** Per BIND-05 the card stays
  in `doing` and the binding is NOT auto-cleared. `card.session_id`
  keeps pointing at the crashed session so the user can navigate to
  the dead pane (read its transcript, debug the failure). The
  `[system] session crashed: …` comment is the only side-effect the
  bridge writes. To retry, the user manually rebinds via PATCH
  (D-10/D-11) — picking a fresh session id, or unbinding to clear the
  slot.

### Area D — Card / session detail surfacing

- **D-13:** **Card detail "Bound session" panel** in
  `dashboard/src/lib/components/BoardItemDialog.svelte`. Sits ABOVE
  the existing comments section. Three rows:
    1. Status pill (`<StatusPill status={session.status}/>` — reuse
       the FleetRow component if it already exists; otherwise inline
       a tiny `<span class="status-pill">`)
    2. Pane tail: `<pre>` with `aria-live="polite"`, 20 lines fetched
       via `GET /api/sessions/{id}/pane?lines=20` on a 2 s
       `setInterval` while the dialog is open. Cancel on close.
       Existing ratelimit on `/pane` already protects the daemon
    3. `<a href="/sessions/{session.id}">Open session →</a>` button
- **D-14:** **Session view back-link** added to the topbar of
  `dashboard/src/routes/sessions/[id]/+page.svelte`. A chip rendered
  only when `session.card_id IS NOT NULL`:
    `← Card #{card.id}${parent_goal ? ` (in “${parent_goal.title}”)` :
    ""}`
  Click navigates to `/board?focus={card.id}` (reuses Phase 1's filter
  pill if present; otherwise highlights via existing scrollIntoView
  pattern).
- **D-15:** **TUI parity keybindings.** In
  `crates/agentum/src/commands/terminal/app.rs`:
    - Board view, card has `session_id`: press `s` → switch to that
      session's pane (`Overlay::Session` or whatever the existing
      board→session jump uses; cross-check during planning — there's
      already a board-row navigation pattern)
    - Session view, session has `card_id`: press `c` → switch to the
      board view, scroll to that card
  Both follow the existing single-letter overlay pattern and respect
  Phase 1's TOOL_SUGGESTIONS / TUI conventions (no clashes with `t`
  for tool cycle, `G` for goal overlay, `S` already used for `Ctrl-S`
  profiles).

  **Amended 2026-05-22 (Phase 2 plan-checker iteration 1):** The TUI has
  no Board view in Phase 2 — the `Focus` enum in
  `crates/agentum/src/commands/terminal/app.rs` is restricted to
  `Tree / Term / TermRight / Lazygit` (no `Board` variant). The board-side
  keybinding (`s` on a card → jump to bound session pane) is therefore
  **deferred** until a TUI Board view ships in a later phase (cross-ref:
  `02-UI-SPEC.md` §Reconciliation lines 277-281 — "future-proofing note:
  when a TUI Board view ships (Phase 3+ or v2), the mnemonic-symmetric
  pair becomes …"). The session-side keybinding (`c` on a focused session
  with `card_id != None` → reveal a one-cell hint strip with the bound
  card id + truncated title) **does ship in Phase 2** as the discoverability
  affordance for the binding.

  Audit trail: this amendment is the explicit record of the half-decision
  drop. The original D-15 wording is preserved verbatim above so the
  Phase 1→2 decision history is auditable. Source for the amendment text
  and the key-collision rationale: `02-UI-SPEC.md` §Reconciliation +
  §Key collision audit (around lines 247-263), which audited the existing
  `s = Stop session` binding (`crates/agentum/src/commands/terminal/app.rs`
  around `:4768`) and concluded `s` MUST NOT be shadowed, while `c` is
  free at the top-level handler.

### Claude's Discretion

The user explicitly invited Claude's judgment on every area for this
phase (same "no more questions" signal as Phase 1). The decisions above
are the inferred best-fit; downstream agents may revise during planning
if a tighter alternative surfaces. Specifically open for revision:

- **D-03 (no opening prompt in v1)** — if the planner reviewer judges
  that BIND-01's "auto-spawn" implies at least a `title + body` first
  message, fold a minimal seed prompt in; otherwise leave the pane
  blank.
- **D-05 (separate bus-subscriber task vs folded into watch_session)**
  — if planning surfaces a simpler one-task design without losing the
  per-session focus, that's fine. The bus subscriber is the safe
  default because it scales the same way `run_goal_reconciler` does.
- **D-13 (20 lines / 2 s polling)** — adjustable based on the existing
  `/pane` rate-limit budget. The TerminalPanel.svelte component may
  already have a streaming WS path (`/api/sessions/{id}/stream`) — if
  that exists and supports a "tail-only" mode cheaply, swap polling
  for streaming.

### Folded Todos

- **`.planning/todos/pending/2026-05-20-board-doing-create-test.md`** —
  *"Add end-to-end test proving a `doing` row can be created via the
  API."* Folded into Phase 2 scope. Natural home: BIND-01's auto-spawn
  is exactly the "create-into-doing" path the todo wants tested. Plan
  as part of the Phase 2 end-to-end test plan (analogous to Phase 1's
  `01-08-PLAN.md`). The todo file moves to `.planning/todos/completed/`
  during the planning step when its `resolves_phase: 2` annotation is
  added (or at execute time via the existing close-phase-todos hook).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 1 outputs (the schema and patterns this phase builds on)

- `.planning/phases/01-goal-cards-planner-slice/01-CONTEXT.md` — Phase 1's
  full context. `session.card_id` and `BoardItem.session_id` were added
  there (SCHEMA-04). Read D-01..D-14 for the architectural shape Phase 2
  extends.
- `.planning/phases/01-goal-cards-planner-slice/01-01-SUMMARY.md` —
  Migration 0015, `Store::add_board_link` / `list_children_of_goal` /
  `max_child_status_rank`, the double-Option `BoardPatch.parent_goal_id`
  pattern that **D-10** copies for `session_id`.
- `.planning/phases/01-goal-cards-planner-slice/01-03-SUMMARY.md` —
  `routes/board_goals.rs` and the `spawn_planner_session` helper that
  **D-01** copies. Includes the "enforce_transition made `pub(crate)`"
  rationale and the atomic create→spawn dual-write template.
- `.planning/phases/01-goal-cards-planner-slice/01-04-SUMMARY.md` —
  `run_goal_reconciler` bus-subscriber task and the
  `planner_stopped: HashSet` idempotency pattern that **D-05** /
  **D-07** copy for `run_session_comment_bridge`.

### Locking specs (slice 1 / slice 2 of the typed-kanban thread)

- `.planning/specs/2026-05-19-typed-kanban-card-schemas.md` — Defines
  `required_fields_for`, `enforce_transition`, and the 400
  `{ missing, status }` payload that **D-02** matches.
- `.planning/specs/2026-05-19-typed-kanban-card-schemas.architecture.md`
  — Names the validation site (server `enforce_transition`). **The
  Phase 2 PATCH still calls `enforce_transition` first; the auto-spawn
  is a side-effect AFTER the gate passes.**
- `.planning/specs/2026-05-20-board-column-rules-overrides.md` — Per-
  server `board_column_rules` table. **If a server has raised the
  `doing` bar above the default (e.g., requires `assignee`), the
  auto-spawn must still respect that gate — gate first, spawn second.**

### Existing codebase files the planner MUST read first

- `crates/agentum-core/src/lib.rs` — `BoardItem`, `BoardPatch`,
  `Session`, `BoardComment` (`author` is free-form, no `kind` column —
  **D-06** uses `"system"` as the author string).
- `crates/agentum-server/src/routes/board.rs` — The PATCH handler at
  `:191`. **D-01** wires the auto-spawn here.
- `crates/agentum-server/src/routes/board_goals.rs` — Phase 1's
  `spawn_planner_session` helper. Template for the new
  `spawn_card_session` helper in **D-01**.
- `crates/agentum-server/src/routes/sessions.rs` — `create` (`:66`),
  `start` (`:220`). The auto-spawn invokes the same launch path
  (`adapter_for(tool).launch()` → `agentum_tmux::new_session`).
- `crates/agentum-watchdog/src/lib.rs` — `watch_session` per-session
  loop and `run_goal_reconciler` bus subscriber. Activity-state
  transitions at `:386-432` are what the bridge subscribes to.
- `crates/agentum-store/src/lib.rs` — Pattern for `Store::claim_card`
  and `Store::transfer_card_binding`. Look at
  `Store::add_board_link` / `update_status_and_target` (which already
  runs a multi-statement transaction) for the transactional shape.
- `crates/agentum-executor/src/lib.rs` + `adapters.rs` — `adapter_for`
  / `ToolAdapter::launch`. No new adapter; the user's `card.tool`
  resolves through the existing registry.
- `dashboard/src/lib/components/BoardItemDialog.svelte` — Where
  **D-13** lands the "Bound session" panel.
- `dashboard/src/routes/sessions/[id]/+page.svelte` — Where **D-14**
  lands the back-link chip.
- `dashboard/src/lib/stores/board.ts` and `sessions.ts` — Stores to
  extend; the existing `events.ts` bus already delivers
  `board.commented` / `agent.*` / `session.crashed`, so the dashboard
  refresh path is already wired.
- `crates/agentum/src/commands/terminal/app.rs` — Where **D-15** lands
  the `s` / `c` keybindings. Cross-reference the existing
  `Overlay::*` and `handle_key` dispatch.
- `crates/agentum-server/src/routes/sessions.rs` (capture-pane route)
  — verify the existing `GET /api/sessions/{id}/pane` shape and
  rate-limit budget before **D-13** picks polling vs streaming.

### Project-level constraints

- `.planning/PROJECT.md` §Constraints — Tech-stack rules, reuse-over-
  rebuild, no-new-SaaS, backwards compatibility (existing cards with
  no `session_id` must keep working — already in place from Phase 1).
- `.planning/REQUIREMENTS.md` §v1 §Binding — BIND-01..BIND-06 are the
  Phase 2 contract.
- `CLAUDE.md` — Rebuild rhythm warning for `dashboard/src/` changes
  (must run `npm run build --prefix dashboard && cargo build
  --release`), the YOLO marker translation rule (the auto-spawn must
  NOT push tool-specific YOLO markers — let the adapter translate, as
  the planner spawn already does), and the conventions around route
  files, sqlx runtime queries, and TUI logging.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`crates/agentum-server/src/routes/board_goals.rs::spawn_planner_session`**
  — Template for the new `spawn_card_session` helper. Already does the
  atomic create-session + start-pane dance. Phase 2's auto-spawn is the
  same shape with `card_id` resolved from the PATCH target instead of
  the goal-create payload.
- **`crates/agentum-store::Store::add_board_link` /
  `Store::max_child_status_rank`** (Phase 1, plan 01-01) — Templates
  for the new `Store::claim_card` and `Store::transfer_card_binding`
  transactional methods. Same sqlx-tx pattern.
- **`crates/agentum-watchdog::run_goal_reconciler`** (Phase 1, plan
  01-04) — Template for `run_session_comment_bridge`. Same
  bus-subscriber shape, same `planner_stopped: HashSet` idempotency
  pattern (D-07 reuses the in-memory dedupe map idea).
- **`agentum_core::BoardComment`** — Existing comment model;
  `author` is free-form string so `"system"` slots in without a
  schema change. Existing route `POST /api/board/{id}/comments` is
  the public mutation path; the bridge writes directly via
  `Store::create_board_comment` (skipping HTTP for in-process speed).
- **`agentum_core::BoardPatch.parent_goal_id` double-Option** (Phase 1,
  plan 01-01) — Template for `BoardPatch.session_id` so the PATCH can
  distinguish "field omitted" vs "explicit null" vs "explicit set".
- **`agentum_server::AppState::bus`** — Single broadcast bus,
  capacity 1024. Phase 2 adds no new event kinds; it consumes
  `agent.awaiting_input` / `agent.finished` / `session.crashed` only.
- **`agentum_executor::adapter_for(tool)`** — Resolves the tool the
  card declares. No new adapter for Phase 2.
- **`GET /api/sessions/{id}/pane`** (existing) — Pane capture for
  **D-13**'s polled tail. Already rate-limited.
- **`Overlay` enum in `crates/agentum/src/commands/terminal/app.rs`**
  — Where new TUI keybindings (`s`, `c`) integrate. Phase 1's
  `Overlay::Goal` lives next to `Overlay::NewSession` and is the
  template for how a new view-state is added.

### Established Patterns

- **PATCH side-effects through `Store::*` transactions.** Phase 1's
  goal-create runs `create_board_item` then `create_session` then
  `update_board_item.session_id` inside `board_goals::create_goal`.
  Phase 2's auto-spawn does the same three steps as
  `Store::claim_card` so the dual-write is atomic.
- **Bus-subscriber background tasks.** `run_goal_reconciler` spawns
  next to `Watchdog::run` from `agentum-server/src/lib.rs::serve`.
  The new `run_session_comment_bridge` joins them.
- **Gate-first, side-effect-second.** `enforce_transition` runs before
  any mutation in `board.rs::patch`. Phase 2 keeps that order intact:
  PATCH validates → gate passes → side-effect (auto-spawn) fires.
- **HTTP error envelope reuse.** `ApiError::Custom(BAD_REQUEST,
  json!({ "missing": [...], "status": "doing" }))` is the shape every
  surfacing pipeline (dashboard + TUI 400-handler) already knows.
- **Watchdog idempotency via in-memory map.** Phase 1's
  `planner_stopped: HashSet<i64>` model is the proven template for
  per-daemon-lifetime dedupe; Phase 2's bridge keeps a similar
  `last_comment_kind: HashMap<Uuid, &'static str>`.
- **Dashboard SvelteKit + rust-embed rhythm.** Any change to
  `dashboard/src/` (D-13, D-14) requires the rebuild incantation in
  CLAUDE.md before the daemon serves the new bundle.

### Integration Points

- **`PATCH /api/board/{id}`** — Existing route; gains the auto-spawn
  side-effect when (status→doing AND session_id=null). No new endpoint.
- **`PATCH /api/board/{id}`** — Also gains explicit
  rebind/unbind via `session_id: <uuid|null>` body field.
- **`run_session_comment_bridge` task** — New tokio task spawned from
  `agentum-server/src/lib.rs::serve`, subscribes to `state.bus`,
  filters to the three agent.*/session.crashed kinds where
  `session.card_id IS NOT NULL AND card.lbl != 'goal'`, inserts a
  `board_comments` row via `Store::create_board_comment`.
- **`dashboard/src/lib/components/BoardItemDialog.svelte`** — New
  "Bound session" panel mounted ABOVE the comments section. Polls
  `/api/sessions/{id}/pane` every 2 s while open.
- **`dashboard/src/routes/sessions/[id]/+page.svelte`** — New
  back-link chip in the topbar when `session.card_id` set.
- **`crates/agentum/src/commands/terminal/app.rs`** — Two new
  keybindings (`s` board→session jump, `c` session→board jump). The
  existing event/overlay dispatch loop is the integration point.
- **`crates/agentum-store::Store::create_board_comment`** — Existing
  method; the bridge calls it directly (skipping the HTTP path) for
  in-process speed.
- **End-to-end test** — Phase 1's `01-08-PLAN.md` is the template for
  Phase 2's final integration plan: drive the full happy path through
  an in-process daemon, assert auto-spawn fires, comment bridge posts,
  PATCH unbind clears, daemon restart preserves binding.

</code_context>

<specifics>
## Specific Ideas

- The **bridge runs in the watchdog crate**, not the server crate.
  Even though `Store::create_board_comment` is a server-ish action, the
  watchdog is the canonical owner of "agent state observation → side
  effect" in this codebase (`run_goal_reconciler` set the precedent).
  Keeps the server's `routes/` folder focused on HTTP surfaces and
  makes the bridge self-contained.
- The **session view's back-link chip uses card title in quotes** when
  rendering the parent-goal context (e.g., `← Card #42 (in "Build the
  auth flow")`). Matches how the dashboard renders goal names
  elsewhere (Phase 1's `lbl=goal` styling + GoalComposer placeholder).
  Keep the chip text short — strip to 40 chars + ellipsis if the goal
  title is long.
- The **`s` and `c` TUI keybindings are deliberately mnemonic-symmetric**
  (s = session, c = card) so users can hop back and forth without
  remembering "did I press the lowercase or capital?". They DON'T need
  modifiers (no Ctrl-S, no Shift). Lowercase, single-keystroke, board
  context only — same scope as the existing card-row navigation.
- The **20-line / 2-second polling cadence** for the pane tail is
  deliberately conservative. It's enough to feel live without
  hammering the daemon. The daemon's existing pane-capture ratelimit
  already absorbs spikes. If the planner reviewer judges streaming WS
  is trivially cheap from the existing `TerminalPanel.svelte` plumbing,
  swap polling for streaming. Otherwise polling is the safe default.

</specifics>

<deferred>
## Deferred Ideas

- **Opening prompt assembly** (card title + body + parent goal +
  `blocked_by` summaries piped into the first agent message). Phase 3
  (UX-01). Phase 2 spawns blank.
- **Preview / edit of the opening prompt before dispatch.** Phase 3
  (UX-02). Same reasoning as above.
- **Dependency-aware column gate** (reject `doing` PATCH when
  `blocked_by` edges are unmet). Phase 3 (GATE-01..04). Phase 2 keeps
  `enforce_transition` body unchanged; the gate already runs, it just
  doesn't know about edges yet.
- **Auto-claim chains / heartbeat reassignment / worktree fanout.** v2
  (AUTO-01..03, WT-01..02).
- **Auto-unbind on session crash.** Per BIND-05 the user decides; the
  binding stays so they can navigate to the dead pane. If real usage
  shows nobody ever wants the dead binding, revisit.
- **Edit / delete board comments.** Noted in `BoardComment` doc-
  comment as a future ask. Keep deferred — render-only fits BIND-04.
- **Streaming WS for the card-detail pane tail.** Polling is the v1
  baseline. Swap to streaming only if the existing `TerminalPanel`
  primitives make it trivially cheap (judged at planning time).
- **Per-card configurable pane-tail length / dashboard setting.**
  Hardcoded 20 lines in v1.
- **Compare-and-swap on board status PATCH**
  (`.planning/todos/pending/board-transition-cas.md`). Phase 3 with
  the dependency gate, same reasoning Phase 1 documented: CAS pays
  off only once the gate exists.
- **A new `BoardComment.kind` enum (`human|system|agent_event|tool`).**
  Tempting, but `author = "system"` covers the BIND-04 surface
  without a schema change. Revisit if v2 wants typed filters on the
  comment stream.

### Reviewed Todos (not folded)

- **`.planning/todos/pending/board-transition-cas.md`** — *"Serialize
  concurrent board status PATCHes (compare-and-swap)."* Reviewed and
  kept deferred. Belongs in Phase 3 alongside the dependency gate
  (matches Phase 1's same-call deferral). Adding a CAS clause to
  `patch_board_item` is small but only pays off once the
  dependency-aware gate exists, otherwise it adds 409s without a
  guarded invariant to protect.

</deferred>

---

*Phase: 02-card-session-binding*
*Context gathered: 2026-05-21*
