# Spec 399 — Observable issue-driven gated workspace

- **Number:** 399
- **Status:** In progress
- **Surface:** `crates/agentum-desktop/ui` + `crates/agentum-server/src/harness*`
- **Author:** Mateo Cerquetella
- **Date:** 2026-07-21
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/399

## Problem

When an operator creates a workspace from a tracker issue and starts a gated SDD
run, the handoff from issue selection to autonomous execution is opaque. The
selected issue is easy to miss, role/QA sessions are not attached to the
workspace, the run collapses to labels such as `blocked · blocked` without its
stage, tasks, or blocker, and the SDD toolbar can disappear because the attached
tab does not carry the engine's agent identity.

## Goal

Keep an issue-driven gated workspace visibly tied to its work and make its live
SDD stage, agent session, task progress, blocker, and controls continuously
observable from creation through completion or intervention.

## Users / personas

- **Workspace operator** — selects or creates an issue in Create Workspace,
  arms “Start gated run,” and then watches or intervenes in the autonomous SDD
  workflow without leaving that workspace.

## Acceptance criteria

1. Create Workspace renders the resolved tracker identity, a searchable open-
   issue list, and an unmistakable selected-issue row/summary; filtering never
   changes the linked issue and issue selection remains optional.
2. Every engine-owned interactive session (PM, architect, feature, browser QA,
   and reviewer) is published as the run's current session and automatically
   appears in the owning workspace while it is active.
3. Tabs attached from a gated run carry the run's validated `agent_tool`, so the
   existing `SddBarGate` renders Spec, Continue, Status, and Loop controls for
   the live engine agent instead of classifying it as a plain shell.
4. The workspace renders a persistent mission-control strip with a human label
   for the current SDD phase, phase attempts, per-task state/progress, and linked
   issue. It never renders duplicate raw labels such as `blocked · blocked`.
5. A blocked role gate reports the stage that blocked and the last gate summary;
   a blocked feature reports its name, retry count, and `last_error`. The final
   idle agent session remains available for inspection/intervention.
6. Session and progress refreshes use the existing `/api/harness/events` stream
   plus status snapshots. No interval, pane-capture polling, or second agent
   launch path is introduced.
7. Existing non-ownership fallback, issue unlink, tracker transitions, gate
   semantics, and teardown of successfully completed sessions remain unchanged.

## Scope & non-goals (YAGNI)

- **In:** tracker picker clarity; harness current-session attribution for all
  agent-played stages; persistent SDD/task progress; blocker explanation; SDD
  toolbar identity on attached gated-run tabs.
- **Out:** changing retry limits or gate verdict rules; adding a new tracker;
  editing/reordering backlog tasks from the strip; retaining sessions after a
  successful stage; replacing the terminal or SDD playbooks.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `TrackerSection` and `applyLinkedWorkItem`
  (`crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx`)
  remain the single issue-selection and persistence path.
- `spawn_agent_into_pane` (`crates/agentum-server/src/routes/sessions`) remains
  the one launch path for every harness agent.
- `useWorktreeHarnessRun` and `/api/harness/events` remain the push-driven run
  discovery and refresh path.
- `SddBarGate` (`components/sdd/SddBar.tsx`) remains the only SDD toolbar and
  continues to target a real server session.
- `GatedRunBar`, `GatedRunSurface`, and the existing pinned server-session tab
  path are extended rather than replaced.

### Build new

- Search and selected-state presentation within the existing tracker section.
- Harness status context for a blocked role gate (`blocked_phase` and
  `gate_summary`) and current-session publication for role/QA agents.
- A compact SDD phase/task ledger and continuous current-session tab sync in
  the existing gated-run workspace strip.

## Risks & invariants

- All spawns continue through `spawn_agent_into_pane`; the UI only observes and
  attaches to sessions.
- The harness gate remains authoritative; surfacing a blocker never advances or
  retries work.
- Harness and terminal updates remain push-based. The UI may re-read a status
  snapshot after an event but must not add a timer loop.
- `agent_tool` is runtime-validated before it is stored as a `TuiAgent`; unknown
  tools degrade to normal live process/title detection.
- The run owns process teardown. Closing or switching a UI tab only detaches the
  view and must not kill the harness session.

## Harness wiring (the gate)

- **feature_list.json entries:** `tracker-picker-clarity`,
  `all-stage-session-surfacing`, `gated-run-mission-control`.
- **`verify.sh` asserts:** tracker filtering/selection helpers; phase and blocker
  derivation; session attribution/status serialization; gated-run markup and
  agent identity; TypeScript build; focused Rust harness tests.
- **`qa.sh` asserts:** create a workspace with a linked issue and gated SDD run,
  observe tracker selection, live role session + SDD bar, task transitions, and
  a useful blocked-state explanation without a blank workspace.

## Open questions

- None. The screenshots and follow-up define the missing operator outcome; the
  PM gate is unblocked.
