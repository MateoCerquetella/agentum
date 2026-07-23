# Handoff — Architect to Developer

- **Spec:** 027-remove-internal-workspace-board
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- A code-grounded architecture covering desktop Tasks, route removal, external-only tracker seams,
  runtime persistence/reconciler removal, legacy schema tolerance, current docs, and build order.
- Five incremental tasks mapped to the five acceptance criteria and named verification methods.

## Acceptance-criteria evidence

- **AC 1:** Component 1 names `TaskPage.tsx`, `board-client.ts`,
  `ProjectTasksPage.tsx`, and its focused structural test.
- **AC 2:** Component 2 names exact route registrations/modules to delete and a real-router 404
  matrix for every retired family.
- **AC 3:** Component 3 removes `TaskSink::Board` and `Store` from normal transition signatures
  while preserving GitHub/Linear results and bounded legacy input handling.
- **AC 4:** Component 4 preserves migrations, removes runtime CRUD/workers, and specifies a
  reopen-and-byte-snapshot store regression.
- **AC 5:** The build order and test matrix require focused checks, the workspace library suite,
  and the desktop production build.

## Verification

- Architecture acceptance-criteria mapping — PASS (every AC has an implementation seam and test)
- Repository seam search — PASS (planned removals and retained external boundaries match current
  source; remaining `/api/board`/`board_items` references are focused compatibility tests)

## Decisions and invariants

- Remove board capability from normal runtime signatures rather than retaining a dormant sink.
- Preserve historical migrations/rows and compatibility-only serialized fields; no durable data is
  deleted.
- Preserve GitHub Projects, Linear kanban, project task routing, one launch path, push streaming,
  and the two-stage harness gate.

## Remaining risks / next action

- Developer must reconcile the already-landed implementation against this plan, run focused gates,
  and correct any drift introduced by later commits before handing to Tester.
- Tracker status update was not attempted because `agentum_report_status` is unavailable in this
  environment; this is non-fatal and should be retried by the release operator if desired.
