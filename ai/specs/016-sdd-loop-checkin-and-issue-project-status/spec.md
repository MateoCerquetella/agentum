# Spec 016 — SDD loop check-in over MCP + issue Project-status in the hover card

- **Number:** 016
- **Status:** Done — F1 slice (SDD-loop check-in), reviewer sign-off 2026-07-13 at `99670cf1`; F2 chip rider split to harness spec 358b, not yet built
- **Surface:** `crates/agentum-server` (routes/sdd.rs, sdd.rs, routes/mcp.rs) + `crates/agentum-desktop/ui` (sidebar hover card)
- **Author:** Claude (from Mateo's ask, GitHub issue #358)
- **Date:** 2026-07-13
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/358

> Code citations are against `origin/develop` @ `bee8dc2d` — this worktree's
> checkout is v0.57.0-era and does NOT contain `routes/sdd.rs`; implementation
> must branch from fresh `origin/develop` (same lesson as the v0.70.0 wizard
> work).

> **PM gate 2026-07-13:** split into two harness specs — F1 (SDD-loop
> check-in) stays in `.agentum-harness/specs/358-the-sdd-loop-sohuld-inject-itself-10-tim/`
> (gated, in flight); F2 (Project-status chip rider) moved to
> `.agentum-harness/specs/358b-issue-hover-project-status-chip/` (pending its
> own PM gate). Bundling both violated the one-slice rule — they share no code
> path or verification surface.

## Problem

Toggling the per-session SDD loop re-injects the orchestrator bootstrap into
the pane on **every settle until a fixed 10-step cap** — the server has no way
to know the work finished. The completion signal lives only in the agent's
reply text ("the SDD loop is complete"), which the server cannot read, so a
spec that finishes on step 2 still gets pounded with up to 8 more "SDD loop
step N" prompts. The transcript fills with near-identical injected prompts, the
agent re-reads state and re-declares completion (or worse, invents new work),
and the session becomes annoying and confusing to follow.

Secondary (rider, added mid-run by Mateo): the sidebar issue hover card shows
only the open/closed state and labels — when the repo is bound to a GitHub
Project, the issue's actual board column (Status: Todo / In Progress / …) is
invisible without opening GitHub.

## Goal

Make the SDD loop stop the moment the agent reports the work done — a pull
check-in over MCP instead of blind re-injection — demoting the 10-step cap to
a safety backstop.

## Users / personas

- **Mateo (self-hosting engineer)** toggles the SDD loop on a workspace agent
  and walks away; he returns to a transcript with ~10 duplicate injected
  prompts fired *after* the spec was done — noise, wasted tokens, and no idea
  which step actually finished the work.
- **The looped agent itself**: each redundant injection is an invitation to
  invent unrequested work; a confused agent burns the remaining steps doing
  damage instead of stopping.
- (F2) **Mateo scanning the sidebar**: hovers a workspace's issue badge and
  wants to see where the ticket sits on the configured GitHub Project board
  without leaving the app.

## Acceptance criteria

### F1 — SDD loop check-in (the slice)

1. A new MCP tool (working name `agentum_sdd_loop`) **accepts**
   `{session, done, summary?}` and, when `done: true` for a session with an
   active SDD loop, **stops** that loop: the worker injects no further steps,
   the handle is removed, and `sdd.loop.stopped` is **emitted** with a new
   reason `agent_completed` (and the step count reached).
2. `sdd::loop_step_prompt` (sdd.rs:165) **instructs** the agent to end every
   step by calling that tool — reporting `done: true` when `ai/STATE.md` says
   the phase is done / there is no actionable next step, `done: false`
   otherwise — replacing the current unread "reply briefly that the loop is
   complete" instruction. The prompt **embeds** the session id so the tool
   call can name it (same explicit-id pattern as `agentum_report_status`,
   mcp.rs:1227 — the MCP layer has no ambient caller identity).
3. `drive_sdd_loop` (routes/sdd.rs:350) **checks** the completion flag after
   each settle and **stops before injecting** the next step when a
   `done: true` check-in arrived during the turn. Loop-stop reasons remain
   named strings on the `sdd.loop.stopped` payload; existing reasons
   (`max_steps`, `settle_timeout`, `session_gone`, `session_not_running`,
   `inject_failed`, `toggled_off`) are unchanged.
4. Belt for MCP-unwired tools (bash/aider — `tool_is_mcp_wired` false, so they
   get the full playbook and cannot call MCP): before injecting step N+1 the
   worker **reads** `ai/STATE.md` in the session's workdir and **stops** with
   reason `state_done` when the current phase is `done`. Missing or unparseable
   STATE.md **falls through** to the existing behavior (inject), never errors
   the loop.
5. A check-in with `done: false` **does not** stop the loop; its `summary`
   lands in the `sdd.loop.step` event payload (progress visible to any client)
   — no other behavior change.
6. The 10-step cap (`DEFAULT_MAX_STEPS`, routes/sdd.rs:60) and the
   settle-timeout stop **remain** as backstops, byte-for-byte: a loop whose
   agent never checks in behaves exactly as today.
7. Unit tests **cover**: done-check-in stops before the next inject;
   `state_done` stops an unwired-tool loop; the step prompt contains the
   check-in instruction + session id; a loop with no check-in still ends at
   `max_steps`.

### F2 — issue Project-status chip (rider, human-directed)

8. When a worktree's repo has a Projects v2 binding configured
   (`routes/github_projects.rs::get_binding`, :322) and the hover card opens
   for a linked GitHub issue, the card **renders** a Project-status chip with
   the issue's current Status option name (e.g. "In Progress") — visually
   distinct from the open/closed `IssueStateBadge` (WorktreeCardMeta.tsx:316)
   and the internal `TrackerPhaseChip` (:320).
9. No binding, issue not on the project, or fetch error → the chip **renders
   nothing** (silent absence, no error state, card otherwise unchanged).
10. The status is **fetched lazily on card open** and cached per issue for the
    app session — hovering does not refetch, and no fetch happens for cards
    never opened.

## Scope & non-goals (YAGNI)

- **In:** the MCP check-in tool + loop-stop wiring; the step-prompt rewrite;
  the STATE.md belt; the hover-card status chip + its one read query.
- **Out:**
  - No change to how the loop is *started* or its UI toggle.
  - No transcript parsing to detect completion (fragile; the check-in replaces it).
  - No immediate mid-turn abort on `done: true` — the loop stops at the next
    settle boundary (simpler; the turn is already ending).
  - No harness-engine changes — `harness::drive` has its own gate loop; only
    the per-session SDD loop (`routes/sdd.rs`) is in scope.
  - (F2) No status *editing* from the hover card; the board already does that.
  - (F2) No Linear equivalent — GitHub Projects only.
- **One-slice note:** F2 is a second slice bundled into this spec at Mateo's
  explicit mid-run request (issue #358's run). It stays an independent
  feature entry so the harness gates it separately.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `routes/sdd.rs` — the whole loop machinery: `SddLoopHandle` (generation,
  step, abort), `run_loop`/`drive_sdd_loop` (:315/:350), `sdd.loop.*` events.
  The check-in only adds a completion flag consulted between settle and inject.
- `sdd.rs::loop_step_prompt` (:165) + its unit tests — edit in place.
- `routes/mcp.rs` tool registry — `tool_specs()` + `call_tool` arm pattern;
  `agentum_report_status` (:1227) is the template for explicit-id session
  addressing and best-effort text results.
- `harness::wait_for_settle` / `SettleOutcome` — untouched; the loop keeps
  using it.
- (F2) `WorktreeCardDetailsHover` (WorktreeCardMeta.tsx:218) — the chip slots
  into the existing badges row (:314–:325).
- (F2) `github_projects.rs::BoardBinding` (:111) + `routes/github_projects.rs::get_binding`
  (:322) — the per-repo binding read.
- (F2) `commands/gh_projects.rs::gh_get_project_view_table` (:766) — the
  `gh api graphql` plumbing pattern for the new single-issue status read.

### Build new

- The `agentum_sdd_loop` MCP tool (spec + `call_tool` arm + a small
  `sdd_loops`-facing stop/report fn in `routes/sdd.rs`).
- A completion flag on `SddLoopHandle` (e.g. `done: Arc<AtomicBool>` + summary
  slot) the tool flips and the worker reads.
- A tiny STATE.md phase reader (workdir-relative, error-tolerant).
- (F2) One GraphQL read — issue number → bound project's Status option name
  (issue → `projectItems` → `fieldValueByName("Status")`) — plus a lazy
  fetch-on-open hook and the chip component.

## Risks & invariants

- **Sacred autonomy mechanics** (architecture_principles.md): the two-step
  `inject_prompt`, settle detection, and YOLO handling are untouched — the
  check-in is additive; AC 6 pins backstop behavior byte-for-byte.
- **MCP gating:** the new tool must be available whenever the MCP master
  switch is on (like `agentum_report_status`, NOT behind the orchestration
  gate) — a looped agent that can't reach the tool degrades to today's
  behavior, never worse.
- **Idempotence/races:** a check-in for a session with no active loop is a
  no-op success (the agent may outlive the loop); a stale generation must not
  stop a successor loop (reuse the existing `generation` guard).
- **Best-effort contract:** tool errors are caller bugs; loop-side failures
  (STATE.md unreadable, event-bus lag) never crash the worker — fall through
  to existing behavior.
- (F2) **GitHub rate limits:** fetch only on card open + cache; silent absence
  on error (AC 9) so a flaky `gh` never degrades the sidebar.

## Harness wiring (the gate)

- **feature_list.json entries:**
  - F1 "SDD loop stops on agent check-in (MCP) instead of blind 10× re-injection"
  - F2 "Issue hover card shows the bound GitHub Project's Status for the issue"
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green,
  including the four AC-7 tests; `cargo fmt --check`; UI build
  (`npm run build --prefix crates/agentum-desktop/ui`) + vitest for the F2
  fetch-cache model.
- **`qa.sh` asserts:** (browser QA) toggle the SDD loop on a session whose
  spec is already done → the loop stops after step 1 with reason
  `agent_completed` (event log), no second injected prompt in the pane; hover
  a worktree's issue badge on a Project-bound repo → the status chip shows the
  board column.

## Open questions

1. **Tool shape:** dedicated `agentum_sdd_loop` (recommended — its semantics
   are loop-control, not tracker phases) vs. a new op on
   `agentum_report_status`? Architect to pin the name + arg schema.
2. **F2 read path:** desktop Tauri command beside `gh_get_project_view_table`
   (gh CLI auth already lives there; popover is desktop-only — recommended)
   vs. a server route beside `get_binding`? Architect to pin.
3. Should `done: true` also wake the settle wait immediately (Notify) instead
   of waiting for the idle signal? Deferred — out of scope (see non-goals)
   unless the architect finds it free.
