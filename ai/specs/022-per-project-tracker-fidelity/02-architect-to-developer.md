# Handoff — Architect → Developer (spec 022)

**Date:** 2026-07-17 · **Mode:** autonomous SDD loop · **From:** Architect · **To:** Developer

## What's ready
- `spec.md` — PM-gated, human-approved handoff, "keep as one spec" confirmed.
- `architecture.md` — edit map (exact `origin/develop` anchors), build order (C → A → B),
  risks + mitigations, and the test/gate strategy.

## Build order & scope (three gated harness features)
1. **C — Open-on-GitHub:** `WorktreeCardMeta.tsx:313` → `onClick={() => api.shell.openUrl(issue.url!)}` (drop the dead `href`/`target="_blank"` branch at that call site only).
2. **A — board-card Status:** draw the Status chip (+ issue-type, "updated X ago") in `ProjectBoardCard.tsx` footer using data already on `row.fieldValuesByFieldId`; thread the board group-field id from `ProjectBoardView`.
3. **B — carry the tracker bind:** `submitQuick` (`useComposerState.ts:2681-2703`) must call `deriveTrackerBindCoords` and pass `trackerProvider`/`trackerUrl` like `submit`; forward those fields in `tauri/worktrees.ts:16-30` + `server-worktree-client.ts:26-38`; fix the false comment at `CreateWorkspaceWizard.tsx:258`. **Server is already wired — don't touch `routes/worktrees.rs`.**

## ⛔ Preconditions before writing code (NEEDS-HUMAN)
1. **File the tracking issue.** #360 shipped v0.78.0 and is closed; this is a *new* follow-up. Issue-first is a hard repo rule; autonomous `gh issue create` has been denied before. Proposed title/labels are in `spec.md` §Open questions. Once filed, paste the URL into `spec.md` frontmatter `tracker:`.
2. **Implement on a fresh develop worktree — NOT `cero`.** This worktree is v0.57.0-era; every anchor above is a `develop` line. Run:
   `git worktree add ../agentum-022-tracker-fidelity -b feat/022-tracker-fidelity origin/develop`
   and do all edits + `verify.sh`/`qa.sh` there.

## Gate to advance Developer → Tester
- `npm run build --prefix crates/agentum-desktop/ui` green; `bunx vitest` for the four new tests (bind payload, adapter whitelist, board-card Status chip, no-`target="_blank"`); `cargo build -p agentum-desktop` compiles.
- Browser QA (`qa.sh`): Status chip visible on a board card; new-workspace issue row shows `#N` + non-null tracker chip; ↗ fires `api.shell.openUrl`.
