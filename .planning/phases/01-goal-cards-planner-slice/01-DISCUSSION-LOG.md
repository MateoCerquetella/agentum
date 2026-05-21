# Phase 1: Goal → Cards (planner slice) - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-20
**Phase:** 1 — Goal → Cards (planner slice)
**Areas discussed:** Goal entity shape & lifecycle, Planner spawn + output mechanism
**Areas deferred to Claude's discretion (user signal: "no more questions, go code"):** Goal composer & parent-cue UI, Planner config + prompt design

---

## Goal entity shape & lifecycle

### Q1 — Data-model shape

| Option | Description | Selected |
|--------|-------------|----------|
| BoardItem with lbl="goal" | Goal IS a regular BoardItem; reuse all existing endpoints, comments, ordering, claim/release. `parent_goal_id` on children points to it. Aligns with PROJECT.md's "no parallel `task` entity" rule. Migration adds one nullable column + the new `board_links` table. Cheapest path. | ✓ |
| Separate `board_goals` table | New table with `(id, title, body, status, ...)` distinct from `board_items`. Forces a parallel UI surface, two endpoints to keep in sync. Conflicts with PROJECT.md's locked decision. | |
| Goal lives only on board_links as a synthetic root | No "goal" row anywhere. Just `board_links(from, to, kind="parent_of")`. Smallest schema, but no place to attach comments / planner session / status. | |

**Notes:** Slice-1 gate passes by default (`todo` needs `title` + `lbl`; both are present on goals).

### Q2 — Goal's column at submission

| Option | Description | Selected |
|--------|-------------|----------|
| `todo` | Goal appears in `todo` next to its children. Zero new columns, zero gate work. Distinction comes from `lbl="goal"` + the new parent-cue UI. | ✓ |
| New `goals` pseudo-column | Per-server column rule + dashboard layout work. | |
| Pinned bar above the board | Strong visual hierarchy but bigger UI scope (new component + layout). | |

### Q3 — Auto-progression based on children

| Option | Description | Selected |
|--------|-------------|----------|
| No — user moves the goal manually | v1 stops at "cards appear, ready to claim". Cheapest. **Was Claude's recommendation.** | |
| Yes — first child to `doing` flips goal to `doing`; all `done` flips goal to `done` | Real productivity gain; needs watchdog hook + graph walk. | ✓ |
| Yes, `done` only | Halfway. | |

**Notes:** User overrode the manual recommendation in favor of full auto-progression. This drove a follow-up question on reverse semantics.

### Q4 — Reverse direction (child reopens after goal flipped done)

| Option | Description | Selected |
|--------|-------------|----------|
| Reverse symmetrically — invariant `goal.status = max(child statuses)` in `{todo<doing<done}` | Same recompute path covers child create / update / delete. One-line rule. | ✓ |
| One-way — goal can auto-advance but never auto-revert | Breaks the invariant; `done` goal with non-`done` children is exactly the anomaly we wanted to fix. | |
| Toast prompt per event | New interaction pattern; risks notification fatigue. | |

**Notes:** The `max()` invariant elegantly captures every edge case (no children → max of empty = `todo`; child deletion → recompute; manual override → transient until next child event).

---

## Planner spawn + output mechanism

### Q1 — How the planner posts cards to the daemon

| Option | Description | Selected |
|--------|-------------|----------|
| `agentum board add` CLI shim | Subcommands `add-goal`/`add-card` wrap `/api/board` calls. Planner calls a clean CLI; no curl boilerplate, no token in scrollback, token resolves via existing `credentials.toml`. | ✓ |
| Direct curl with token in env | Zero new code, but agents are bad at constructing JSON; token leaks into pane logs. | |
| Pane-output parsing with structured marker | Watchdog scrapes `<CARD>...</CARD>` blocks. Fragile (markers can split across capture boundaries); watchdog grows scope. | |
| New `/api/board/bulk` endpoint with single payload | Atomic but still needs a curl invocation unless paired with the CLI shim — degenerate hybrid. | |

### Q2 — Planner spawn timing + lifecycle

| Option | Description | Selected |
|--------|-------------|----------|
| Spawn on goal submit; auto-stop when planner writes the first child (or `<DONE>` marker) | Goal endpoint atomically creates goal + spawns planner; `session.card_id = goal.id`; watchdog detects first child or `<DONE>` and idles the session. Goal retains `session_id` for retrospection. | ✓ |
| Spawn on goal submit; planner stays alive indefinitely | User-managed cleanup; idle sessions consume context budget. | |
| Pre-spawn one shared `planner` session per server; goals sent as messages | Breaks the 1:1 card↔session model; planner context pollution between goals. | |
| One-shot subprocess outside tmux | Contradicts PROJECT.md's locked decision (planner runs as a tmux session). | |

### Q3 — How edges (`blocked_by`) get specified

| Option | Description | Selected |
|--------|-------------|----------|
| Sibling-key references on each `add-card` call (`--key oauth ... --blocks oauth`) | Planner invents local keys; daemon resolves against children created under the same goal. Forward-refs buffer briefly. | ✓ |
| Real AG-keys only — planner reads stdout from prior calls | Cleanest contract but LLMs are bad at exact key capture across shell rounds. | |
| Single bulk JSON file | Atomic but agents are bad at multi-card JSON; one malformed file blocks the whole goal. | |
| No structured edges in v1 | Conflicts with SCHEMA-02 + Phase 3 gate. | |

### Q4 — CLI shim auth

| Option | Description | Selected |
|--------|-------------|----------|
| Shim reads `~/.config/agentum/credentials.toml` (existing TUI auth flow) | Zero new env vars; no secrets in planner prompt or scrollback; refuses with a one-line hint if missing. | ✓ |
| Daemon injects `AGENTUM_TOKEN` env var into the planner pane | Works without `credentials.toml` but token can leak via `env` output. | |
| Unix socket with SO_PEERCRED | Big architectural addition for one-feature payoff. | |
| Anonymous loopback shortcut with per-session key | Brand-new trust mode; only worth it if option 1's UX hurts. | |

---

## Claude's Discretion

User said "no more questions, go code" after Area 2. The following areas were
not interactively walked through; defaults below were inferred from existing
codebase patterns + PROJECT.md constraints and recorded in CONTEXT.md as
D-09…D-14.

### Area 3: Goal composer & parent-cue UI

- **D-09** Dashboard composer: persistent `GoalComposer.svelte` bar above the
  board (Hermes-inspired; matches PROJECT.md framing). No modal.
- **D-10** Parent-cue on child cards: a small chip `↳ AG-42` in the card
  header; click to open the goal. No grouping/indent in v1. A filter pill on
  the column header lets the user scope the column to one goal's children.
- **D-11** TUI overlay: a new `Overlay::Goal` (mirroring `Overlay::NewSession`),
  reachable via a new keybinding (suggested `G`) from the Board view. Single
  multiline editor; Enter submits.

### Area 4: Planner config + prompt design

- **D-12** New `$XDG_CONFIG_HOME/agentum/planner.toml`. Resolution order:
  `prompt_file` → `prompt` → bundled default. Daemon reads on every goal
  submit (no cache in v1).
- **D-13** Default planner prompt baked via `include_str!` from
  `crates/agentum/src/commands/board/planner_prompt.md`. Names the CLI
  surface, explains `--key`/`--blocks`, ends with `<DONE>`. Zero-config UX.
- **D-14** `planner.tool` defaults to `"claude"`; resolved via
  `agentum_executor::adapter_for()` so any first-class adapter Just Works.

These defaults are open to revision in the planning step if a better-fitting
approach surfaces. They are not user preferences.

## Deferred Ideas

- Phase 2 (BIND-01..06) — claim → auto-spawn flow, `card.session_id ↔
  session.card_id` bidirectional binding, watchdog events → card comments.
- Phase 3 (GATE-01..04) — dependency-aware column gate that reads
  `board_links(blocks)`; compare-and-swap on PATCH (from
  `board-transition-cas.md`).
- Phase 3 (UX-01..03) — editable opening prompt, preview-before-dispatch,
  card-comment-thread agent-activity interleave.
- v2 — auto-claim chains, full-graph hands-off execution, heartbeat
  reassignment, worktree fanout, personas.
- Sub-goals / goal threading (depth > 1) — explicitly one-level in v1.
- Per-workdir `planner.toml` overrides — daemon-wide only in v1.
- Dashboard / REST UI for editing `planner.toml` — file-on-disk only in v1.
- Folded todo `2026-05-20-board-doing-create-test.md` — better fit for Phase
  2 BIND-01 verification; recorded as a Phase 1 deferred-ideas tail rather
  than a Phase 1 blocking task.
