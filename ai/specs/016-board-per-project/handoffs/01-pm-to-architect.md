# Handoff 01 — PM → Architect (spec 016, 2026-07-13, autonomous)

**Gate verdict:** PASS (sdd-pm sub-agent), contingent amendments APPLIED to
`spec.md` (AC 3 hint append, AC 6 replacement, verify.sh grep-line
replacement, Open Questions → Locked decisions D1–D3).

## What the architect receives

- `spec.md` — Status: PM. AC 1–7 final. Decisions D1 (pick-wins + divergence
  hint), D2 (bare openers → `openProjectHub(repoId,'tasks')` → `openProjectsPage()`
  fallback, gate preserved), D3 (`activeView === 'tasks'` branch stays — it is
  live for detail openers / internal nav / history replay).
- "PM risks for the architect" section in spec.md — 6 items, all code-verified
  on this worktree (rebased to origin/develop v0.75.1, HEAD `4662cf42`+amends).

## Architect must produce

`ai/specs/016-board-per-project/architecture.md`:
1. The settings-shape change (`activeProjectByRepo` on `GitHubProjectSettings`)
   + the stable-shape default (`shared/constants.ts:317`) + persistence path.
2. The pure resolver module (name, signature, location under `ui/src/lib/`,
   test file) — precedence pick → binding → legacy global; consumed by
   ProjectViewWrapper, ProjectPicker display prop, and the hub effect.
3. Repo-context threading design: how embedded TaskPage → ProjectViewWrapper /
   ProjectPicker learn the repo (prop vs store selector) — must answer PM risk
   2 (where `githubMode:'project'` forcing lives) and risk 3 (surgical
   commitSelection split).
4. Hub effect retarget (`ProjectHubPage.tsx:82-123`): per-repo write, `hostId`
   threading, no global write.
5. Sidebar removal + D2 re-route table (every caller, old → new).
6. File-by-file build plan (F1/F2/F3 slices per spec harness wiring) with the
   verify gate per slice.

Constraints: UI-only (no Rust edits); `useComposerState` untouched;
`CreateWorkspaceWizard.tsx:136-140` fallback behavior preserved (may adopt the
resolver, must not regress spec 012); vite build + vitest are the gates (no
bare tsc — `shared/*` is a vite alias).

## Environment notes (orchestrator)

- Worktree rebased onto origin/develop v0.75.1 (`253173ad`) BEFORE any code
  work; diagnosis re-verified on new HEAD.
- `bun install` done; baseline `bun run build` GREEN (1m25s, exit 0).
- vitest has a known pre-existing failing baseline repo-wide — new suites must
  be green in isolation; full-run failures must be proven pre-existing.
