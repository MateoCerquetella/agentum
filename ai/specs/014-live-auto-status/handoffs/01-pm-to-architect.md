# Handoff 01 — PM → Architect (spec 014-live-auto-status)

- **Date:** 2026-07-09
- **From:** PM (sdd-pm, autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Verdict:** PM gate **PASS** (all 9 validate_handoff.md boxes; evidence in
  the PM report, summarized below). Spec `spec.md` amended in place (Status →
  PM, 8 amendments applied).

## What the Architect receives

`ai/specs/014-live-auto-status/spec.md` — four ordered, independently
gateable slices closing the auto-status visibility + agent-attention loop:

1. **F1 `tracker-phase-event`** — every successful tracker transition emits
   `tracker.phase_changed` on the existing global bus / `/api/events` WS.
2. **F2 `phase-chip-live`** — `tracker_phase` exposed in the worktrees API +
   a live phase chip on the workspace card (pure derivation model, vitest).
3. **F3 `board-live-refresh`** — event-driven debounced re-fetch of the
   Projects-mode board (2 s coalesce window, named constant; no polling).
4. **F4 `attention-signal`** — `session.crashed` / sustained
   `agent.awaiting_input` (≥ threshold) in a bound worktree →
   `apply_blocked_transition`; auto-clear on recovery via idempotent
   current-phase re-apply.

## PM-locked decisions (do NOT re-open; cheap for Mateo to veto later)

- **D1** — attention threshold **10 min** default via
  `AGENTUM_ATTENTION_AFTER_SECS`; threshold-only, **no focus-awareness** v1.
- **D2** — **auto-clear on recovery**; at most ONE blocked comment per
  stuck-episode; crash-loop comment cooldown **60 min** (named constant) —
  label may re-apply idempotently, duplicate comments suppressed.
- **D3** — **one spec, no F4 pre-split**; F4 ordered LAST in
  feature_list.json; execution-time escape hatch: demote F4 to spec 015 on a
  red gate with zero rework.
- (AC-7 numeric lock) — board re-fetch coalesce window **2 s**, named
  constant, only while the Projects view is visible.

## Open decisions delegated to YOU (the only two)

- **Q1 — emission point:** inside `apply_tracker_transition` (bus handle
  threaded into task_sink) vs at its call sites (`tracker_sync.rs:153/:379`,
  `harness/drive.rs:388`). PM constraints: only SUCCESSFUL transitions emit;
  fire-and-forget; weigh which choice makes "transition without emitting" the
  harder future mistake.
- **Q5 — event vocabulary:** one `tracker.phase_changed` with a
  `blocked: bool` flag vs a distinct `tracker.blocked` event. PM constraint:
  the F2 chip must derive its attention state from the event payload alone
  (no follow-up fetch).

## Standing obligations

- **Re-ground first.** This worktree is **stale at v0.57.0**; every
  `path:line` in spec.md was verified against `origin/develop` v0.67.0
  (`8fb7eb16`) via `git show`/`git grep`. Re-verify each cited line on fresh
  `origin/develop` before writing architecture.md; implementation must happen
  on a branch based off `origin/develop`, not this tree's base.
- **Invariants:** never-halt/best-effort writes; monotonic guard untouched
  (`status/blocked` stays orthogonal, no new TrackerPhase variant); no
  polling reintroduction (F3 is event→re-fetch); fail-closed on unbound
  worktrees; worktree registry serde-alias-FREE; one launch path untouched.
- **Deliverable:** `ai/specs/014-live-auto-status/architecture.md` —
  boundaries, seam choices for Q1/Q5 with tradeoffs, module placement (server
  worker siting for F4 next to `tracker_sync.rs`, UI file placement next to
  `WorktreeCardMetadataStatusBadges.tsx` / `ProjectViewWrapper.tsx`), test
  strategy per slice (fake-`gh` subprocess pattern from task_sink tests;
  jsdom-free vitest models), and build order.

## Known accepted residuals (no action needed)

- In-memory sustained-awaiting timer resets on embedded-server restart (an
  in-flight episode is forgotten) — acceptable under best-effort; note it in
  architecture.md if relevant.
- GitHub read-after-write lag can make the F3 re-fetch land before GitHub is
  consistent — accepted v1, no retry loops (Risks section).
