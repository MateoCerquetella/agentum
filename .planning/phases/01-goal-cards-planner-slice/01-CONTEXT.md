# Phase 1: Goal → Cards (planner slice) - Context

**Gathered:** 2026-05-20
**Status:** Ready for planning

<domain>
## Phase Boundary

A user submits a single goal (dashboard composer OR TUI overlay). The daemon
atomically creates a goal card and spawns a dedicated planner tmux session
bound to it. The planner agent decomposes the goal into 3–7 child kanban
cards with `parent_goal_id` and `blocked_by` edges, emitting each card via
a new `agentum board add-*` CLI shim. Every connected client sees the
goal card and its children land in `todo`, with a visible parent-cue on
each child. Cards persist across daemon restart with edges intact.

**In scope:**
- New nullable `parent_goal_id` column on `board_items` (SCHEMA-01)
- New `board_links(from_card_id, to_card_id, kind)` table (SCHEMA-02), with `kind ∈ {parent_of, blocks}`
- Numbered migration `0015_orchestrator.sql` (SCHEMA-03), nullable + additive
- New nullable `card_id` column on `sessions` (SCHEMA-04)
- `agentum-core` type updates: `BoardItem.parent_goal_id`, `Session.card_id`, new `BoardLink` (SCHEMA-05)
- Dashboard `GoalComposer.svelte` above the board (ORCH-01)
- TUI `Overlay::Goal` reachable via new keybinding from the board view (ORCH-02)
- New endpoint that atomically creates the goal card + spawns the planner session (ORCH-03)
- Planner produces 3–7 children within ~2 min, each with `parent_goal_id` set and edges where appropriate (ORCH-04)
- Per-server `planner.toml` under `$XDG_CONFIG_HOME/agentum/` with `tool` + `prompt`/`prompt_file`; falls back to `claude` (ORCH-05)
- Dashboard + TUI render the goal distinctly via `lbl="goal"` styling, and child cards show a parent-cue chip (ORCH-06)
- New `agentum board add-goal` / `agentum board add-card` subcommands (the planner's output surface)
- Watchdog-driven goal status auto-progression: `goal.status = max(child statuses)` in the order `todo < doing < done`

**Out of scope (explicitly deferred):**
- Claiming a card / auto-spawning a session on `todo → doing` — that's Phase 2 (BIND-01..06)
- Dependency-aware column gates that enforce `blocked_by` on PATCH — that's Phase 3 (GATE-01..04)
- Auto-claim chains / hands-off full-graph execution — v2
- N:N card↔session binding (locked decision: 1:1 only)
- A parallel `task` entity alongside `BoardItem` (locked decision: extend the existing schema)
- The planner being an in-process LLM call (locked decision: tmux session only)
- A REST UI for editing the planner prompt — `planner.toml` is the only editor surface in v1
- Per-workdir scoping of the planner config — daemon-wide only

</domain>

<decisions>
## Implementation Decisions

### Goal entity shape & lifecycle

- **D-01:** The "goal" IS a `BoardItem` with `lbl = "goal"`. No parallel
  `board_goals` table. Reuses every existing endpoint, comment, ordering
  and claim/release path. Aligns with PROJECT.md's locked decision
  ("extend the existing schema, don't introduce a second concept").
- **D-02:** The goal card lands in the `todo` column at submit time,
  next to its children. Distinguished visually by the `lbl="goal"` badge
  (existing lbl styling) and the new parent-cue UI (see D-09). No new
  columns are introduced.
- **D-03:** The goal card **auto-progresses** with its children. Invariant:
  `goal.status = max(child statuses)` in the order `todo < doing < done`.
  - First child to move to `doing` flips the goal to `doing`.
  - Last child to move to `done` flips the goal to `done`.
  - **Reverses symmetrically.** If a child moves back from `done` to
    `doing`, the goal moves back from `done` to `doing`. If the last
    `doing` child moves back to `todo`, the goal moves back to `todo`.
  - **Empty-children goal** (no rows where `parent_goal_id = goal.id`)
    defaults to `todo` (max of empty set is the lowest rank).
  - **Child deletion** triggers the same recompute (child removed from
    the set). The watchdog (or a dedicated reconciler) is the only
    writer of auto-goal-status — never a client PATCH.
- **D-04:** The user can still manually drag the goal card. A manual
  PATCH that disagrees with `max(child statuses)` wins for the duration
  of that PATCH, but the next child transition recomputes and may
  overwrite. (Auto wins on every child event; manual is a transient
  override.) **No locking, no "pinned" flag** — keeps the rule one-line
  and respects the v1 "ready to claim, no auto-execution" envelope.

### Planner spawn + output mechanism

- **D-05:** The planner's output surface is a new CLI: `agentum board
  add-goal --title ...` and `agentum board add-card --parent-goal AG-X
  --title ... --key <local> --blocks <local-key>,...`. Each subcommand
  is a thin wrapper over the existing `/api/board` (extended) and
  `/api/board/links` (new) endpoints. **Not curl in the prompt; not
  pane-output parsing.** Rationale: agents are good at calling clean
  CLIs, bad at producing exact JSON or capturing AG-keys across shell
  rounds; pane-parsing makes the watchdog do too much.
- **D-06:** **Sibling-key references for edges.** The planner invents
  symbolic local keys (`--key oauth`) in the same goal-submission run
  and references them on subsequent calls (`--blocks oauth`). The
  daemon resolves symbolic keys against children already created under
  the same `parent_goal_id`. Forward-references buffer for a short
  window (one planner session). Unknown keys → CLI exits non-zero with
  `unknown sibling key: foo`, and the planner agent's job is to fix
  its own call.
- **D-07:** Planner lifecycle: **spawn on goal submit, auto-stop when
  the planner emits the goal's first child** (or earlier, if the planner
  agent exits cleanly or prints a `<DONE>` marker, whichever fires
  first). The watchdog watches for the first row where
  `parent_goal_id = <this_goal_id>` and emits a
  `goal.planner.first_child` event. The planner session retains its
  `card_id` link to the goal (`session.card_id = goal.id`) so the user
  can `agentum tail` to inspect the planner's reasoning later. Status
  flips to `Idle` after the auto-stop; user can re-run by manually
  starting the session again.
- **D-08:** The CLI shim authenticates by reading
  `~/.config/agentum/credentials.toml` (the file the TUI already
  reads/writes via `crates/agentum/src/commands/auth.rs`). The shim
  uses the `local` profile by default; refuses to run with a one-line
  setup hint if `credentials.toml` is missing. **No new env vars, no
  tokens in the planner's prompt or scrollback.**

### Goal composer & parent-cue UI (Claude's discretion — see Discretion block below)

- **D-09:** **Dashboard composer** is a new `GoalComposer.svelte` that
  sits as a persistent input bar above the board (single textarea +
  "Plan it" button). Matches the PROJECT.md framing
  ("the kanban *is* the orchestrator: a goal in, executing cards out").
  Reuses the same store-action shape as `BoardItemDialog.svelte`. No
  modal. On submit it POSTs to the new `/api/board/goals` route, then
  the existing `events` WS delivers the new rows.
- **D-10:** **Parent-cue on child cards** is a small chip in the card
  header: `↳ AG-42`. Clicking the chip navigates to (or pops up) the
  parent goal card. **No grouping or indent in v1** — the column
  renderer stays flat; goals and their children share the column. A
  filter pill next to the column header (`Filter: AG-42 ↓`) lets the
  user scope the column to one goal's children when they want focus.
- **D-11:** **TUI overlay.** A new `Overlay::Goal` (mirroring
  `Overlay::NewSession`) reachable from the Board view via a new
  keybinding (TBD by the planner; suggested `G` since lowercase `g`
  often means "first row"). Single multiline text editor + Enter to
  submit. Goal card + child cards stream in via the existing
  `/api/events` WS, so the TUI's board panel updates as the planner
  works.

### Planner config + prompt design (Claude's discretion — see Discretion block below)

- **D-12:** Per-server planner config lives in a **new
  `$XDG_CONFIG_HOME/agentum/planner.toml`** (sibling to `profiles.toml`
  and `credentials.toml`). Schema:
  ```toml
  [planner]
  tool = "claude"           # default; any value resolvable by adapter_for()
  prompt_file = "/etc/.../planner.md"   # OR
  prompt = "<inline prompt text>"        # OR
  # If neither is set, fall back to a bundled default baked into the binary.
  ```
  Resolution order: `prompt_file` → `prompt` → bundled default. The
  daemon reads `planner.toml` on each goal submit (no in-memory cache
  in v1; rules table pattern). Missing file = use bundled defaults.
- **D-13:** **Default planner prompt is baked into the binary** via
  `include_str!("planner_prompt.md")` from
  `crates/agentum/src/commands/board/planner_prompt.md`. The prompt
  names the `agentum board add-goal` / `add-card` CLI surface, explains
  `--key` / `--blocks` semantics with a worked example, and ends with
  "emit `<DONE>` when finished". Keeping it bundled means the feature
  works out of the box with zero setup; users can override by setting
  `planner.prompt_file`.
- **D-14:** `planner.tool` defaults to `"claude"`. The CLI shim resolves
  the tool via the existing `agentum_executor::adapter_for(tool)`
  registry, so any first-class adapter (`claude`, `codex`, `cursor`,
  `gemini`, `hermes`) Just Works as a planner.

### Claude's Discretion

The user stopped the interactive discussion after Areas A (Goal entity
shape & lifecycle) and B (Planner spawn + output mechanism), saying "no
more questions, go code". The following areas were not interactively
walked through — decisions D-09 through D-14 above are Claude's best
inference from the codebase patterns and PROJECT.md constraints, NOT
the user's explicit preferences. They are open to revision in the
planning step if the planner agent surfaces a better-fitting approach
or the user pushes back on review.

- **Area C: Goal composer & parent-cue UI** (D-09, D-10, D-11) — bias
  toward a persistent input bar over a modal, parent-cue as a chip in
  the card header, no grouping/indent in v1.
- **Area D: Planner config + prompt design** (D-12, D-13, D-14) — bias
  toward a new dedicated `planner.toml`, a bundled default prompt so
  the feature works zero-config, and `tool="claude"` as the default.

### Folded Todos

- **`2026-05-20-board-doing-create-test.md`** — *"Add end-to-end test
  proving a `doing` row can be created via the API."* Folded into
  Phase 1's deferred-ideas tail. **Caveat:** the user folded it during
  the cross-reference step, but its natural home is Phase 2 (BIND-01
  covers exactly the "create-into-doing via auto-spawn" path). Plan as
  Phase 1 deferred → reclassify in Phase 2 plan if it lands there
  naturally; do NOT block Phase 1 verification on it.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Locking specs (slice 1 + slice 2 of the typed-kanban thread)

- `.planning/specs/2026-05-19-typed-kanban-card-schemas.md` — Slice 1.
  Defines `required_fields_for` const, `enforce_transition` gate, the
  grandfathering rule, the 400 `{missing, status}` payload shape, and
  the `transition out` always-allowed rule. **The orchestrator MUST
  produce goal/child cards that satisfy whatever `todo` requires
  (currently `title` + `lbl`).**
- `.planning/specs/2026-05-19-typed-kanban-card-schemas.architecture.md`
  — Slice 1 architecture. Names the validator API, validation site
  (server `enforce_transition`), event payload (`board.transition.rejected`),
  and the five risks. **Risk #2 (CAS race on concurrent PATCH) is
  deferred to Phase 3, not Phase 1.**
- `.planning/specs/2026-05-20-board-column-rules-overrides.md` — Slice
  2. Per-server `board_column_rules` table; replace-not-merge semantics;
  custom-column passthrough is bypass-by-default; auth: any daemon
  token can edit rules. **The orchestrator must respect whatever rule
  the user has set for `todo` on a given server; if `PUT
  /api/board/rules/todo` raised the bar above `title + lbl`, the
  goal card POST will 400 and the daemon must surface that to the
  composer.**
- `.planning/specs/2026-05-20-board-column-rules-overrides.architecture.md`
  — Slice 2 architecture (read on demand during implementation).

### Existing codebase files the planner MUST read first

- `crates/agentum-core/src/lib.rs` — `BoardItem`, `NewBoardItem`,
  `BoardPatch`, `Session`. The shape being extended.
- `crates/agentum-core/src/board_schema.rs` — `required_fields_for`,
  `validate_transition`, `RequiredField::as_missing_key()`. The gate
  the orchestrator must respect.
- `crates/agentum-server/src/routes/board.rs` — Existing CRUD +
  `enforce_transition` call site. The new `goals` route + new
  `links` route will live here (or in a sibling `board_goals.rs` /
  `board_links.rs` per project convention — `routes/mod.rs` is a
  flat list).
- `crates/agentum-server/src/routes/sessions.rs` — `create` → `start`
  flow that the new `POST /api/board/goals` will call into to spawn
  the planner pane.
- `crates/agentum-server/src/routes/board_rules.rs` — Sibling pattern
  for an admin-scope route file (slice 2 just landed; use as a template
  for the new goal/link routes).
- `crates/agentum-executor/src/lib.rs` — `ToolAdapter` trait,
  `adapter_for(tool)`, `LaunchCommand`. The planner uses
  `adapter_for(planner_tool)` exactly like a user-spawned session does.
- `crates/agentum-executor/src/adapters.rs` — Existing adapter
  patterns. The orchestrator does NOT need a new adapter — it just
  spawns the user's configured tool with a custom first-message prompt.
- `crates/agentum-store/src/lib.rs` — `Store` add `add_board_link`,
  `list_children_of_goal`, `delete_board_link`, plus extending the
  board-item CRUD to carry `parent_goal_id`.
- `crates/agentum-store/migrations/0014_board_column_rules.sql` —
  Template for the new migration's comment style (slice 2 explained
  the denormalised-JSON rationale up top; the new migration should
  comment on why `board_links` is a separate table vs an inline JSON
  column on `board_items`).
- `crates/agentum-watchdog/src/lib.rs` — `watch_session` per-session
  loop. The goal-status recompute hooks into the same event flow.
- `crates/agentum/src/commands/auth.rs` — How the TUI reads
  `credentials.toml`. The new `agentum board add-*` subcommands reuse
  the same path resolution.
- `crates/agentum/src/commands/terminal/app.rs` — Where the new
  `Overlay::Goal` lands (alongside `Overlay::NewSession`,
  `Overlay::Settings`, etc., per the existing `Overlay` enum at
  `app.rs:158`).
- `crates/agentum-store/src/paths.rs` — XDG path helpers. Add a
  `planner_config_path()` for `$XDG_CONFIG_HOME/agentum/planner.toml`.
- `dashboard/src/lib/components/BoardItemDialog.svelte` — Pattern
  template for `GoalComposer.svelte` (props shape, store binding, error
  surfacing).
- `dashboard/src/lib/stores/board.ts` — Where the new
  `submitGoal(text)` action lands.

### Project-level constraints

- `.planning/PROJECT.md` §Constraints — Tech stack rules, reuse over
  rebuild, no new SaaS dep, backwards compatibility (`parent_goal_id`
  must be nullable), embedded SPA rebuild rhythm.
- `.planning/REQUIREMENTS.md` §v1 — SCHEMA-01..05, ORCH-01..06 are the
  Phase 1 contract.
- `CLAUDE.md` — Project conventions, the rebuild rhythm warning, the
  YOLO marker translation (the planner spawn must NOT push a tool-
  specific YOLO marker; let the adapter translate as today).

### External (Hermes shape, not codebase)

- `https://hermes-agent.nousresearch.com/docs/user-guide/features/kanban`
  — UX inspiration for the goal-input-bar above the board. We adopt
  the **shape**, not the codebase.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`agentum_core::BoardItem`** — already serde-compatible with both
  the API and the dashboard wire. Adding `parent_goal_id:
  Option<i64>` with `#[serde(default, skip_serializing_if =
  "Option::is_none")]` is backwards-compatible for all existing
  consumers.
- **`agentum_core::board_schema::RequiredField` enum** — the
  canonical name space for field identifiers. If the goal-create
  flow ever needs to surface "missing parent_goal_id" on a child,
  add a new variant here (NOT a magic string).
- **`agentum_server::routes::board_rules`** — pattern template for
  admin-scope routes. New goal/link endpoints follow the same
  signature shape (`Result<Json<T>, ApiError>` + `ApiError::Custom`
  for the 400 `{missing, status}` envelope).
- **`agentum_server::AppState::bus`** — single `broadcast::Sender<Event>`
  with capacity 1024. The new `goal.planner.first_child`,
  `goal.planner.complete`, and `goal.status.changed` events ride this
  bus; dashboard's existing `/api/events` WS picks them up free.
- **`agentum_executor::adapter_for(tool)` + `ToolAdapter::launch()`**
  — the planner-spawn flow uses these identically to a user-spawned
  session. No new adapter.
- **`agentum::commands::auth::credentials_path()` + `load_credentials()`**
  — the new `agentum board add-*` subcommands reuse this to discover
  the local daemon's token. (Confirm the exact function names while
  planning — they may be `read_token_for_profile()`.)
- **`Overlay::NewSession` in `crates/agentum/src/commands/terminal/app.rs`**
  — template for the new `Overlay::Goal`. Same `apply_event` / `handle_key`
  dispatch shape, same modal lifecycle (`Esc` to cancel, `Enter` to submit).
- **`BoardItemDialog.svelte`** — template for `GoalComposer.svelte`.
  Same fetch/error pattern; props differ.

### Established Patterns

- **Numbered, nullable, additive migrations.** `0015_orchestrator.sql`
  follows `0001..0014` exactly: filename pattern, raw SQL, default
  preserves existing-row behavior, included via
  `sqlx::migrate!("./migrations")`.
- **Runtime sqlx queries only** (`sqlx::query`, `sqlx::query_as`); no
  compile-time `query!` macros so CI doesn't need a live DB. New
  queries follow this rule.
- **Route handler shape:** `pub fn router() -> Router<AppState>`;
  handlers small (5–20 lines); literal paths before `{id}` dynamic
  segments; canonical extractor order `State<AppState>, Path<…>,
  Query<…>, Json<…>`.
- **HTTP error envelope:** default `{ "error": msg }`; gate failures
  use `ApiError::Custom(StatusCode::BAD_REQUEST, json!({ "missing":
  [...], "status": "doing" }))` — match this shape for new
  validation paths.
- **TUI never `eprintln!`** — `tracing::info!` only;
  `init_tracing_for_tui()` routes to the cache log file.
- **Dashboard SvelteKit + rust-embed rhythm:** any dashboard change
  needs `npm run build --prefix dashboard && cargo build --release`
  before the daemon serves the new bundle. Phase 1 plans must include
  this in any task touching `dashboard/src/`.

### Integration Points

- **`POST /api/board/goals`** (new) — atomic create-goal +
  spawn-planner. Calls `Store::create_board_item(lbl=goal, ...)` →
  `Store::create_session(card_id=goal.id, tool=planner_tool, ...)` →
  `agentum_executor::adapter_for(planner_tool).launch(...)` →
  `agentum_tmux::new_session(...)`. Returns the new goal `BoardItem`
  to the client; planner output flows back via the events bus.
- **`POST /api/board/links`** (new) — `{from_card_id, to_card_id, kind}`;
  used by `agentum board add-card --blocks ...` after the daemon
  resolves the symbolic key.
- **`GET /api/board/links?goal=AG-42`** (new) — used by the dashboard
  to render the parent-cue chip and (optionally) the column filter.
- **Watchdog hook on child status transitions** — extend
  `watch_session`'s status-emit path to additionally:
  1. If the row being updated has `parent_goal_id IS NOT NULL`,
     recompute the parent's `max(child statuses)` via a single
     `SELECT MAX(status_rank) FROM board_items WHERE parent_goal_id = ?`
     query (status_rank is `CASE status WHEN 'todo' THEN 0 WHEN 'doing'
     THEN 1 WHEN 'done' THEN 2 ELSE -1 END`).
  2. If the recomputed rank differs from the goal's current rank,
     issue an internal `update_status` on the goal and emit
     `goal.status.changed`.
  3. **No recursion** — goals don't have parents in v1, so the recompute
     stops at depth 1.

</code_context>

<specifics>
## Specific Ideas

- The **persistent goal-input bar** above the board is explicitly
  modeled on Hermes Agent Kanban's UX (cited in PROJECT.md). The
  user does not want a modal-only entry point; the composer should
  feel like a "drop the goal in, watch the cards land" affordance.
- The **`max(child statuses)` invariant** for auto-progression is the
  single rule that defines the goal lifecycle. The watchdog (not the
  HTTP layer) owns this — keeps it in one place and avoids the HTTP
  layer needing to know about goal cards specifically.
- The **planner is a normal agent session** that happens to be bound
  to a goal card via `session.card_id`. No special status, no special
  adapter — it can be `claude`, `codex`, `gemini`, etc., depending on
  `planner.toml`.
- The **`agentum board add-*` shim is the planner's only output
  surface.** The prompt MUST teach the planner this; we do not want
  the planner inventing curl invocations or pasting JSON into the pane.

</specifics>

<deferred>
## Deferred Ideas

These came up during analysis or were considered but explicitly belong
in later phases / out of scope. Do not pull them into Phase 1.

- **Auto-spawn a tool session when a child card moves to `doing`.**
  Phase 2 (BIND-01). Phase 1's planner spawn is the only auto-spawn.
- **Dependency-aware column gate** (reject `doing` PATCH when
  `blocked_by` edges are unmet). Phase 3 (GATE-01..04). Phase 1
  creates the `board_links` rows; Phase 3 reads them on transition.
- **Editable opening prompt + preview before dispatch.** Phase 3
  (UX-02). Phase 1's planner spawn uses the bundled / configured prompt
  as-is.
- **Per-workdir scoping of `planner.toml`.** Daemon-wide only in v1.
  If a real need surfaces, add a `[planner.<workdir>]` override block
  later — not now.
- **Dashboard UI for editing `planner.toml`.** Edit by hand. A future
  settings panel can wrap this once the feature stabilises.
- **REST endpoint for editing `planner.toml`.** Same as above —
  edit-on-disk only in v1.
- **Goal threading / sub-goals (goals with parents).** v1 explicitly
  one-level: goals have children; children do not have grand-children.
  The recompute loop relies on this assumption (depth=1).
- **Compare-and-swap on board status PATCH**
  (`.planning/todos/pending/board-transition-cas.md`). Reviewed during
  cross-reference; deferred to Phase 3 alongside the dependency gate
  work (both are PATCH-concurrency concerns).

### Reviewed Todos (not folded)

- **`board-transition-cas.md`** — *"Serialize concurrent board status
  PATCHes (compare-and-swap)."* Reviewed but kept deferred. Belongs in
  Phase 3 where the gate work happens. Adding a CAS clause to
  `patch_board_item` is a small change but only pays off once the
  dependency-aware gate exists, otherwise it adds 409s without a
  guarded invariant to protect.

</deferred>

---

*Phase: 01-goal-cards-planner-slice*
*Context gathered: 2026-05-20*
