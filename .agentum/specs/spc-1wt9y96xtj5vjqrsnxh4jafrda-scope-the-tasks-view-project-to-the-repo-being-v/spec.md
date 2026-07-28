---
schema: 1
id: SPC-1WT9Y96XTJ5VJQRSNXH4JAFRDA
revision: 1
title: Scope the Tasks-view project to the repo being viewed
source: legacy-import:.agentum-harness/specs/396-projects-should-be-splitted-by-projects/spec.md@sha256:129b199a2b09ab32c05625b15a73f1df890fbbbbc82cccd2d7ba767b129433fe
---

# Scope the Tasks-view project to the repo being viewed

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 396 — Scope the Tasks-view project to the repo being viewed
>
> - **Status:** PM (refined at the PM gate)
> - **Surface:** `crates/agentum-desktop/ui`
> - **Source:** https://github.com/MateoCerquetella/agentum/issues/396
> - **Date:** 2026-07-20
>
> > Raw issue: "Projects SHOULD BE splitted by projects, also in configuration. …
> > We shouldn't be able to see same kanban/linear project if its attached to X
> > and we are seeing Y." ("kanban project" = the GitHub Projects board in the
> > Tasks view; "configuration" = the persisted UI resume state.)
>
> ## Problem
>
> The Tasks view remembers the last-viewed GitHub Project / Linear project in one
> **global** slot, so after switching from repo X to repo Y the user still sees
> repo X's project — and can drag cards or file issues into the wrong tracker.
>
> ## Goal
>
> When a user switches the viewed repo, the Tasks view shows the tracker project
> bound to *that* repo (or the picker when none is bound) — never another repo's.
>
> ## Users / personas
>
> Mateo, a developer running agentum across several repos (workspaces), each with
> its own GitHub Project or Linear project. The moment it bites: he jumps from
> repo X's workspace to repo Y's and the board still shows X's project as he
> reaches to move a card.
>
> **User value:** the board always shows the project that belongs to the repo you
> are looking at — never another repo's.
>
> ## Acceptance criteria
>
> - [ ] Selecting a GitHub Project or Linear project while viewing repo X **persists** the binding under repo X's scope key, and the binding survives an app reload.
> - [ ] Switching the view to repo Y **renders** repo Y's bound project, or **renders** the project picker/empty state when Y has no binding; repo X's project never renders while viewing repo Y.
> - [ ] Un-selecting the project while viewing repo X **removes** only repo X's binding; repo Y's binding still restores on next visit.
> - [ ] A `taskResumeState` blob written by an older (global) build **hydrates** without crashing, and the next project the user views is stored under the scoped key (backward-compatible migration).
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** the Tasks view's GitHub-Project ("kanban") and Linear-project modes; the persisted resume/binding configuration for them, keyed by viewed repo.
> - **Out:** the built-in server kanban — `GET /api/board` lists all items globally (`routes/board.rs:55`) and tracker bindings are global (`routes/board_sync.rs:173`); scoping it is a follow-up slice. Out: GitLab projects, re-syncing already-mirrored cards, new providers, any server schema change.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `TaskResumeState` + `PersistedUIState` (`ui/src/shared/types.ts:2452`, `:2470`) — the global resume blob; add scoping here.
> - `setTaskResumeState` (`ui/src/store/slices/ui.ts:1006`) — persists via `api.ui.set`; the one write path to extend.
> - Scope key: `lastActiveRepoId` / `lastActiveWorktreeId` (`types.ts:2471-2472`) — the same repo the sidebar shows as viewed.
> - Selection flows: `ProjectPicker` (`components/github-project/ProjectPicker.tsx`) and Linear select/restore (`components/TaskPage.tsx:976`, `:1082`).
>
> ### Build new
>
> - Per-repo keying of the project binding (both providers share the one slot today) + hydration/migration of the legacy global blob.
>
> ## Risks & invariants
>
> - Hydration must tolerate legacy/tampered persisted JSON (existing contract, `types.ts` persisted-state comments).
> - The prefetch path reads `taskResumeState` globally (`ui.ts:958-1004`) — keep it working.
> - UI-state-only slice: no routes, no auth, no spawn paths — harness invariants untouched.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** one — "Per-repo tracker-project binding in the Tasks view".
> - **`verify.sh` asserts:** UI slice tests for scoped persist/switch/clear/migrate + `npm run build --prefix crates/agentum-desktop/ui` green (no server change expected).
> - **`qa.sh` asserts:** browser pass — bind project to repo X, switch to repo Y (picker/empty), switch back (X's project returns), reload (bindings persist).
>
> ## Open questions
>
> - None blocking: the scope key defaults to the viewed repo (`lastActiveRepoId`); the architect may widen it to worktree granularity if cheaper.
