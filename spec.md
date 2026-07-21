# Spec 399 — Observable issue-driven gated workspace and authoritative tracker status

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
tab does not carry the engine's agent identity. For GitHub-linked worktrees the
sidebar also presents two competing lifecycle states: the live GitHub Project
Status and Agentum's persisted `trackerPhase`. A stale local `In Progress` can
therefore render beside an authoritative external `TODO`, making neither status
trustworthy.

## Goal

Keep an issue-driven gated workspace visibly tied to its work, make its live SDD
stage, agent session, task progress, blocker, and controls continuously
observable, and make GitHub Project Status the sole displayed and authoritative
lifecycle state for GitHub-linked worktrees.

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
8. A tracker phase is persisted locally only after the provider returns
   `Applied`. `Skipped` and transport failures remain pending, are visible in
   logs, and retry with bounded backoff; detecting a pull request never consumes
   the only attempt to move its issue into In Review.
9. When a legacy project binding has no In Review option ID, the first PR
   transition re-discovers the bound Status field. A matching `In Review`
   option is persisted and used automatically; boards without one retain their
   documented In Progress fallback.
10. A GitHub-linked issue's sidebar hover card renders exactly one lifecycle
    chip: the current Status option name read from the bound GitHub Project. It
    never renders `TrackerPhaseChip`, even when local and external values match.
11. Opening/loading a linked GitHub issue revalidates its Project Status instead
    of trusting a session-long snapshot. A stale registry `trackerPhase` is
    ignored for presentation and reconciled from the external value; in
    particular local `in_progress` plus external `TODO` renders only `TODO` and
    cannot preserve a false local success.
12. GitHub lifecycle writes target configured Project option IDs and preserve
    their option names: work start → In Progress, PR open/link → In Review,
    testing → Ready to Test when configured, and merge/completion → Done.
13. A lifecycle transition is acknowledged and cached locally only after GitHub
    returns `Applied`. `Skipped` and errors trigger an external-status refetch,
    remain pending for bounded retry, and surface an actionable sync warning;
    the UI never invents the requested local status.
14. Regression coverage proves: stale local In Progress + external TODO renders
    one TODO chip; matching local/external values still render one chip; a
    skipped/failed transition cannot persist false success; PR open/link writes
    In Review; and stale registry records reconcile from GitHub.

## Scope & non-goals (YAGNI)

- **In:** tracker picker clarity; harness current-session attribution for all
  agent-played stages; persistent SDD/task progress; blocker explanation; SDD
  toolbar identity on attached gated-run tabs; authoritative GitHub Project
  Status reads/writes, stale-registry reconciliation, and visible sync failure.
- **Out:** changing gate retry limits or gate verdict rules; adding a new tracker;
  editing/reordering backlog tasks from the strip; retaining sessions after a
  successful stage; replacing the terminal or SDD playbooks; using the local
  pipeline phase as a second GitHub lifecycle state.

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
- `gh_issue_project_status` and `getProjectBinding`
  (`crates/agentum-desktop/src/commands/gh_projects.rs` and
  `crates/agentum-desktop/ui/src/runtime/github-projects-client.ts`) remain the
  external read path; their result, not `trackerPhase`, drives the GitHub chip.
- `apply_tracker_transition` and the configured `StatusMapping`
  (`crates/agentum-server/src/task_sink.rs` and `github_projects.rs`) remain the
  only GitHub Projects write path and preserve renamed option IDs/names.

### Build new

- Search and selected-state presentation within the existing tracker section.
- Harness status context for a blocked role gate (`blocked_phase` and
  `gate_summary`) and current-session publication for role/QA agents.
- A compact SDD phase/task ledger and continuous current-session tab sync in
  the existing gated-run workspace strip.
- A single external-status view model with explicit loading/synced/warning
  outcomes, cache invalidation on linked-issue open and tracker events, and no
  GitHub rendering dependency on `deriveTrackerChip` or `TrackerPhaseChip`.
- Registry reconciliation that accepts the option ID from the live desktop
  GitHub read, validates it against the server-owned binding, persists only the
  confirmed phase, and emits enough pending/error context for the sidebar to
  explain how to retry authentication, binding, or status-option failures.

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
- **Single source of truth:** for `trackerProvider === "github"`, Project Status
  is authoritative. `trackerPhase` may remain as an implementation/cache field
  for retry guards, but cannot be rendered, cannot override a fetched status,
  and cannot advance unless the matching external status is confirmed.
- A failed refetch must retain the last externally confirmed value and mark it
  stale/warning; it must not fall back to the desired local transition.
- Configured Project option IDs are authoritative, so renamed labels such as
  `In Review` are displayed exactly as GitHub returns them rather than rebuilt
  from Agentum's canonical phase names.

## Harness wiring (the gate)

- **feature_list.json entries:** `tracker-picker-clarity`,
  `all-stage-session-surfacing`, `gated-run-mission-control`,
  `authoritative-github-tracker-status`.
- **`verify.sh` asserts:** tracker filtering/selection helpers; phase and blocker
  derivation; session attribution/status serialization; gated-run markup and
  agent identity; single GitHub status chip and warning states; stale registry
  reconciliation; confirmed-only transition persistence; PR→In Review; TypeScript
  build; focused UI and Rust tracker/harness tests.
- **`qa.sh` asserts:** create a workspace with a linked issue and gated SDD run,
  observe tracker selection, live role session + SDD bar, task transitions, and
  a useful blocked-state explanation without a blank workspace; force local
  `in_progress` while the bound Project says `TODO` and observe only `TODO`;
  simulate a rejected Projects write and observe a sync warning with no false
  local phase.

## Open questions

- None. The screenshots and follow-up define the missing operator outcome; the
  PM gate is unblocked.
