# Handoff — Developer to Tester

- **Spec:** 025-issue-first-new-work
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-22
- **Gate:** PASS

## Delivered

- React-free New Work launch model with source/mode, eligibility, contextual
  labels, ordered stages, retry position, and issue checkpoint reuse tests.
- New Work wizard presents New issue / Existing issue and SDD Autopilot / Open
  manually, defers issue filing to the final action, and renders progress.
- `submitQuick` accepts explicit linked issue, execution mode, checkpoint, and
  progress callbacks. It reuses a completed worktree on Retry.
- Autopilot directly requires `start-work` ownership and never opens a plain
  agent on failure. Manual prepares with `plan:false, converge:true` before the
  existing single-agent activation.
- `spec-from-issue` has opt-in converge semantics plus `specExisted`; absent or
  false retains the established 400-on-existing behavior.

## Verification evidence

- Focused Vitest: **PASS** — 6 files, 106 tests.
- `git diff --check`: **PASS**.
- Vite production build: **PASS** — 7,239 modules, completed in 2m41s.
- Focused Rust converge contract: **PASS** — 1 passed, 0 failed, 787 filtered.

## Acceptance-criteria coverage

- **AC 1–2:** staged source + final issue resolver + returned confirmed summary.
- **AC 3–4:** explicit execution cards and mandatory mode-owned spec path.
- **AC 5:** strict `startGatedWork` ownership suppresses the plain agent only
  after ownership is confirmed.
- **AC 6:** manual `plan:false, converge:true`, then unchanged activation.
- **AC 7:** issue/worktree checkpoints and ordered progress survive retry within
  the still-open modal.
- **AC 8:** pure eligibility reasons block Autopilot instead of degrading it.

## Tester priorities / remaining risk

1. Exercise new/existing + Autopilot/manual paths in the installed app,
   including injected post-issue and post-worktree failures.
2. Confirm the final modal layout remains usable at the supported minimum
   height; this developer environment did not perform browser QA.

No commits were created. `ai/STATE.md` was intentionally not edited by the
Developer; phase advancement remains the orchestrator's responsibility.
