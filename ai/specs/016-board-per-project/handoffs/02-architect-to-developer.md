# Handoff 02 — Architect → Developer (spec 016, 2026-07-13, autonomous)

**Gate verdict:** PASS (orchestrator). `architecture.md` is the binding build
plan — implement it as written; log any deviation with justification.

## Build order & discipline

- Implement **F1 → F2 → F3** exactly per `architecture.md` §6 (file tables).
  Run each slice's gate BEFORE starting the next; one git commit per green
  slice (`feat(ui): spec 016 F<n> — <slug>`), no attribution trailers.
- All work under `crates/agentum-desktop/ui/src`. ZERO Rust edits.
- The **must-NOT-touch list** (§6) is binding: useComposerState*,
  ProjectBindingEditor, CreateWorkspaceWizard + wizard models
  (`deriveTrackerBindingTarget` is import-only), server crates,
  worktree-nav-history, TaskPage detail-nav internals, the hub
  `taskDataSeeded` gate/effect.
- Line numbers were verified 2026-07-13 on this HEAD but re-locate before
  editing (anchor on the quoted code, not the number).

## Gates (run from `crates/agentum-desktop/ui`)

- F1: `bun run build` + `bunx vitest run src/lib/board-project-resolution.test.ts`
  + `bunx vitest run src/components/new-workspace/work-item-picker-model.test.ts
  src/components/new-workspace/create-workspace-wizard-model.test.ts`
- F2: build + F1 vitest set + grep: no `updateSettings`/`setTaskResumeState`
  left in `ProjectHubPage.tsx`
- F3: build + `bunx vitest run src/lib/board-route.test.ts
  src/lib/board-project-resolution.test.ts` + the spec grep gate
  (`openTaskPage(` callers outside TaskPage.tsx/tests = detail-payload only)

## Environment (orchestrator-verified)

- `bun install` done; baseline `bun run build` GREEN (1m25s).
- vitest full-suite has a pre-existing failing baseline repo-wide — gate on
  the TARGETED files above; if you run the full suite, prove any failure
  pre-existing before ignoring it.
- No bare `tsc` (`shared/*` is a vite alias).
