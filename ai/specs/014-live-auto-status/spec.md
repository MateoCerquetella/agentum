# Spec 014 — Auto-status you can SEE: live tracker phase inside agentum + agent-attention signal

- **Number:** 014
- **Status:** PM             <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-server` (bus event on transition, watchdog→blocked bridge) + `crates/agentum-desktop/ui` (live phase chip, Projects-board live refresh)
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-09

> **Grounding caveat (read first).** This spec was researched from the
> `how-can-we-make-it-auto-status` worktree, which is **based on v0.57.0** and
> is missing specs 009–013. ALL research below was done against
> **`origin/develop` (v0.67.0, `8fb7eb16`)** via `git show` / `git grep` —
> every `path:line` refers to that ref, not this working tree. The Architect
> must re-ground on fresh `origin/develop` before build (spec 012's rule).
> **Reuse 012/010 over rebuild is a hard rule here** — the *write* side of
> auto-status already shipped; this spec closes the *visibility and
> agent-activity* loop.

## Problem

Mateo picks an issue when creating a workspace and agentum already moves it on
GitHub as he works (spec 012: In Progress on session start, In Review on PR,
Done on merge) — but **inside agentum nothing shows it**. The server's
`tracker_phase` is exposed nowhere in the UI, no event fires on a transition,
and the Projects-mode board only updates on a manual refresh — so the app that
*causes* the status changes can't display them. Worse, between those milestones
the issue can lie: an agent can sit crashed or stuck awaiting input for hours
while the issue still says "In Progress", and nobody watching the board knows a
human is needed.

## Goal

Close the auto-status loop **inside agentum**: every tracker transition emits a
live event the UI renders (phase chip on the workspace card, Projects board
refresh), and the watchdog's crashed/stuck signals reach the bound issue as the
existing `status/blocked` attention flag — so the board Mateo watches is the
board that's true.

## Users / personas

- **Mateo (solo operator), at two moments:**
  1. *Watching the cockpit* — an agent is coding against a picked issue; he
     glances at the workspace card / Board page and wants to see the card's
     pipeline phase (Todo → In Progress → In Review → Done) move **live in the
     app**, without alt-tabbing to github.com or hammering refresh.
  2. *Away from the pane* — an autonomous agent crashes or blocks on input at
     2am. He wants the issue (and the in-app card) to flip to a visible
     "needs attention" state so the stuck run is discoverable from the board,
     not by scrolling terminals.

## Acceptance criteria

Ordered slices F1 → F4; each independently gateable. All tracker writes remain
best-effort/never-halt and go through the ONE existing seam
(`apply_tracker_transition` / `apply_blocked_transition`) — this spec adds **no
new label/Projects/Linear write code**.

**F1 — A transition emits a bus event**

1. Every **successful** tracker phase transition for a bound worktree
   (session-start reactor, PR/merge poller, harness drive call sites) emits one
   `tracker.phase_changed` event on the global broadcast bus carrying at least
   `{worktree_id, provider, phase, tracker_url}` — asserted by a lib test that
   subscribes to the bus and drives a fake-`gh` transition.
2. A **failed or skipped** transition emits nothing (the bus never lies), and
   emission never blocks or reorders the transition itself (fire-and-forget).
3. The event reaches clients over the **existing** `/api/events` WS (the same
   bus the watchdog `agent.*` events already ride) — no new socket, no new
   route.

**F2 — The workspace card shows the phase, live**

4. The worktrees API payload exposes the persisted `tracker_phase` (wire form:
   `todo | in_progress | in_review | ready_to_test | done`) for a bound
   worktree; unbound worktrees expose nothing (fail-closed).
5. The workspace card renders a **tracker-phase chip** (distinct from the
   existing agent-activity dot and open/closed badges) sourced from that field,
   and updates it in place when a `tracker.phase_changed` event for its
   worktree arrives — no page refresh, no new poll. Pure chip-derivation model
   (event/payload → chip state) is jsdom-free with `bunx vitest run` green.
6. A workspace with no picked issue renders **no** chip (never a fabricated
   phase).

**F3 — The Projects-mode board updates itself**

7. While the Board page's Projects mode is visible, an incoming
   `tracker.phase_changed` triggers a **debounced re-fetch** of the active
   project view (reusing the existing `gh_get_project_view_table` read path;
   events inside a 2 s coalesce window — a named constant, not user config —
   cause exactly ONE fetch, asserted by the pure debounce-model vitest) —
   the moved card is visible in-app without pressing refresh. No
   `setInterval`-style background poll is introduced; hidden/inactive views
   fetch nothing.

**F4 — Crashed/stuck agents flag the issue (attention signal)**

8. A `session.crashed` event for a session in a **bound** worktree fires
   `apply_blocked_transition` (the existing `status/blocked` label + one
   explanatory `gh issue comment`) — asserted by a fake-`gh` test. Best-effort:
   a failed write logs and never halts the watchdog loop.
9. A **sustained** `agent.awaiting_input` (continuously awaiting for ≥ a
   configurable threshold, default 10 minutes; `AGENTUM_ATTENTION_AFTER_SECS`)
   in a bound worktree fires the same blocked signal **once** per
   stuck-episode (an episode STARTS when awaiting persists past the threshold
   and ENDS when any AC-10 clear condition fires; no repeat label/comment
   within an episode). Transient prompts answered within the threshold fire
   nothing.
10. The signal **clears**: on `agent.working` / `agent.input_resolved` /
    session restart, the current pipeline phase is re-applied through
    `apply_tracker_transition` (idempotent), which already removes
    `status/blocked` on any pipeline write — the board can't stay stale-red.
    Asserted by a fake-`gh` test (blocked set → working → blocked label
    removed). Crash-loop guard: a NEW blocked episode for the same session
    within a comment cooldown (default 60 min, a named constant) re-applies
    the `status/blocked` label (idempotent) but suppresses the duplicate
    comment — asserted by a fake-`gh` crash-loop test (two crashes inside the
    cooldown ⇒ label present, exactly ONE comment).
11. Blocked/cleared state changes emit a bus event (`tracker.blocked` or a
    flagged `tracker.phase_changed`) so the F2 chip shows/clears a
    "needs attention" variant live.

## Scope & non-goals (YAGNI)

- **In:** the four slices above — a bus event on every successful transition;
  `tracker_phase` exposed to and rendered live by the UI; event-driven
  Projects-board refresh; watchdog crashed/stuck → the existing blocked
  signal + live clear. GitHub-first (Linear gets the same event emission for
  its InProgress arm, but the attention signal is GitHub-label-only, matching
  `apply_blocked_transition` today).
- **Out:**
  - **Rebuilding spec 012/010.** The picker, session-start reactor, PR/merge
    poller, monotonic guard, label/Projects/Linear writes, and
    `done_closes_issue` all exist — this spec only *observes* them (F1–F3) and
    *feeds* them (F4).
  - **Two-way sync.** An external edit to the issue's `status/*` label or
    Projects column is still not read back into `tracker_phase`; the monotonic
    guard stays authoritative. (The Projects board view still shows GitHub's
    truth — it re-fetches from GitHub.)
  - **Progress issue-comments & checkbox check-off.** The CLAUDE.md rule
    ("▶ starting / 🧪 gate running / ✅ green" comments + checking acceptance
    boxes) remains unimplemented — it's a write-volume/noise decision that
    deserves its own spec (candidate 015), not a rider here.
  - **Webhooks.** Still no inbound webhooks; F3 is event-driven re-fetch of an
    already-pull-based view, not a new poll and not push-from-GitHub.
  - **New TrackerPhase variants.** "Needs attention" is the existing
    orthogonal `status/blocked` flag, NOT a sixth pipeline phase — the
    monotonic rank (`Todo<InProgress<InReview<ReadyToTest<Done`) is untouched.
  - **Renaming/reworking the sidebar's user-defined `workspaceStatus` kanban**
    (drag-drop lanes) — it stays manual and separate.

## Reuse vs build (ground in code — all refs = `origin/develop` @ v0.67.0)

### Already exists — do NOT rebuild

- **The entire write seam:** `apply_tracker_transition`
  (`crates/agentum-server/src/task_sink.rs:813`) — labels
  (`GITHUB_STATUS_LABELS:277`, one `gh issue edit` add+remove), Projects
  Status-column write (spec 010, `github_projects::board_write_with:844`),
  Linear `transition_issue` (`linear.rs:322`), internal-board arm. Any pipeline
  write already clears `status/blocked` (`task_sink.rs:518`).
- **The attention write:** `apply_blocked_transition` (`task_sink.rs:890`) —
  `status/blocked` label (`:290`, fixed, non-pipeline) + one
  `gh issue comment` with a `<details>` tail (`blocked_comment_body:568`).
  Today called only by harness retries-exhausted (`harness/drive.rs:322`).
- **The signal source:** watchdog events `agent.working` / `agent.finished` /
  `agent.awaiting_input` / `agent.input_resolved` / `session.crashed`
  (`crates/agentum-watchdog/src/lib.rs:471–546, 338/364`).
- **The bus-subscriber bridge pattern:** `comment_bridge.rs:19`
  (`run_session_comment_bridge`) — event filter, per-session dedupe map,
  lag-tolerant loop. F4 is a sibling worker following this exact shape,
  targeting the tracker seam instead of the internal board.
- **Worktree binding + phase persistence:** `TrackerWorktree`
  (`routes/worktrees.rs:111` — `tracker_provider/tracker_url/tracker_phase/
  linked_pr/branch`), `find_tracker_worktree_by_path` (used by
  `tracker_sync.rs:142`), `persist_tracker_progress` (`worktrees.rs:179`),
  monotonic guard `next_phase_write` (`tracker_sync.rs:76`).
- **Client event plumbing:** the `/api/events` WS + the UI's existing
  consumption of watchdog `agent.*` events
  (`use-worktree-activity-status.ts`, `WorktreeActivityStatusIndicator.tsx`) —
  the push channel and the listen pattern for F2/F3 are already there.
- **Board read path:** `gh_get_project_view_table` + `ProjectViewWrapper.tsx`
  (fetch on project/tab change + manual refresh, effects at `:161/:197/:321`)
  — F3 adds one more trigger to this existing fetch, not a new reader.
- **Agent self-report interface:** the `agentum_report_status` MCP tool
  (`routes/mcp.rs:1201`) already lets any agent push a phase manually — the
  "how do agents interface with status" answer that needs no new code.

### Build new

- **`tracker.phase_changed` emission** — thread the bus (or an emit callback)
  to the transition call sites (`tracker_sync.rs:153/:379`,
  `harness/drive.rs:388`) or emit inside the seam; Architect picks the seam
  point. Today `tracker_sync.rs` emits nothing (verified: no `bus.send` /
  `Event::new` in the file).
- **`tracker_phase` in the worktrees API payload + shared TS type** — grep
  shows `trackerPhase`/`tracker_phase` appears **nowhere** in
  `crates/agentum-desktop/ui` today.
- **Tracker-phase chip** on the workspace card + pure derivation model (new
  files beside `WorktreeCardMetadataStatusBadges.tsx`).
- **Projects-board event-triggered debounced re-fetch** (a `useEffect` hook in
  `ProjectViewWrapper.tsx` keyed on the events store).
- **Watchdog→tracker attention worker** — a new bus subscriber (sibling of
  `comment_bridge.rs`) with the sustained-awaiting timer, per-episode dedupe,
  and the clear-on-recovery re-apply. Lives server-side next to
  `tracker_sync.rs`; no watchdog-crate changes expected beyond consuming its
  events.

## Risks & invariants

- **Never-halt, best-effort (012 invariant #3).** Event emission and every F4
  write must be fire-and-forget: a slow/failed `gh` call cannot stall the
  watchdog loop, the poller, or session start. No transition may become
  conditional on a bus send.
- **Monotonic guard is sacred.** F4's clear re-applies the *current* phase
  (idempotent, rank-equal) — it must never advance or regress the phase.
  `status/blocked` stays orthogonal; no new `TrackerPhase` variant.
- **No poll reintroduction.** F3 is event→re-fetch of the existing pull view.
  Do not add an interval; do not touch pane streaming (push-based invariant).
- **Label churn / API noise.** Sustained-awaiting must be debounced +
  per-episode deduped, else an interactive session (agent prompts every turn)
  spams `status/blocked` on/off. The threshold default (10 min) and the
  clear-on-`agent.working` rule are the churn guards; fake-`gh` tests assert
  no back-to-back duplicate writes.
- **Lossy bus ⇒ stale chip.** The broadcast bus drops events under lag; the
  F2 chip treats events as hints layered over the persisted `tracker_phase`
  (AC 4) — any worktrees re-fetch reconciles. The event stream is never the
  only source of truth.
- **GitHub read-after-write lag.** The Projects view table can lag the
  label/column mutation, so the debounced re-fetch may land before GitHub is
  consistent and show an unmoved card. Accepted for v1 (manual refresh
  remains; the 2 s coalesce window absorbs most of it) — do NOT add
  retry/poll loops for this.
- **Fail-closed binding (012 invariant #5).** Unbound worktree ⇒ no event
  consumers fire, no chip renders, no blocked write — silent no-ops
  everywhere.
- **Worktree registry stays serde-alias-FREE** (spec 004 lesson) if any field
  is added; prefer exposing the existing `tracker_phase` without new fields.
- **One launch path untouched.** Nothing here goes near
  `spawn_agent_into_pane`, YOLO translation, or pane provisioning.

## Harness wiring (the gate)

- **feature_list.json entries** (one shippable slice each):
  1. `tracker-phase-event` — bus emission on successful transitions (AC 1–3).
  2. `phase-chip-live` — API exposure + live card chip (AC 4–6).
  3. `board-live-refresh` — event-driven Projects re-fetch (AC 7).
  4. `attention-signal` — crashed/stuck → blocked → clear (AC 8–11).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green (bus-
  emission test; fake-`gh` blocked-on-crash, sustained-awaiting-debounce,
  clear-on-recovery, failure-never-halts tests) **AND**
  `bun run build --prefix crates/agentum-desktop/ui` succeeds **AND**
  `bunx vitest run` green for the chip/refresh pure models. No bare `tsc`
  (shared/* is a vite alias).
- **`qa.sh` asserts (browser, real board):** create a workspace picking an
  issue → start the agent → the card's phase chip flips to **In Progress**
  without a refresh and the Projects board view moves the card after the
  debounce → kill the agent's pane (simulated crash) → the GitHub issue gains
  `status/blocked` + a comment, and the card chip shows **needs attention** →
  restart the session → the blocked label clears and the chip returns to
  In Progress. Evidence = screenshots per step (browser-verification-loop) +
  the issue's timeline.

## Open questions

1. **Emission point:** emit `tracker.phase_changed` inside
   `apply_tracker_transition` (needs a bus handle threaded into task_sink) or
   at its three call sites (duplication risk, but zero seam change)?
   *Architect call.*
2. **Sustained-awaiting threshold + attended-ness — LOCKED (PM D1):** 10 min
   default, configurable via `AGENTUM_ATTENTION_AFTER_SECS`; threshold-only,
   NO focus-awareness in v1 (it would add a desktop→server focus channel for
   marginal gain, and a false flag while Mateo is attending costs one
   auto-clearing label).
3. **Clear semantics — LOCKED (PM D2):** auto-clear on recovery
   (`agent.working` / `agent.input_resolved` / session restart) via the
   idempotent phase re-apply — a stale red flag after recovery is the same
   board-lie this spec exists to kill. Crash-loop flap is contained by the
   AC-10 comment cooldown (label re-applies silently; comments capped).
4. **F4 separability — LOCKED (PM D3):** F1–F4 ship as ONE spec (repo
   convention: 012 shipped 4 slices; every slice here is independently
   gateable and F4 — the only GitHub-writing slice — runs LAST in
   feature_list order). Execution-time escape hatch: if F4 blocks red, demote
   it to spec 015 with zero rework — F1–F3 have no dependency on it.
5. **Event vocabulary:** one `tracker.phase_changed` with a `blocked: bool`
   flag vs a distinct `tracker.blocked` event. *Architect call*, with one PM
   constraint: either shape must let the F2 chip derive its attention state
   from the event payload alone (no follow-up fetch), preserving AC 5's pure
   derivation model.
