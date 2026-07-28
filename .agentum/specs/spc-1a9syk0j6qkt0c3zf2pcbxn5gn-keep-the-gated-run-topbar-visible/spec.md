---
schema: 1
id: SPC-1A9SYK0J6QKT0C3ZF2PCBXN5GN
revision: 1
title: Keep the gated-run topbar visible
source: legacy-import:spec.md@sha256:e8b9178e91f769f05c64b9d46d9ae58f72556756495884ad06a47de4690c3c62
---

# Keep the gated-run topbar visible

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

> # Spec 437 — Keep the gated-run topbar visible
>
> - **Status:** Done
> - **Surface:** `crates/agentum-desktop/ui`
> - **Tracker:** https://github.com/MateoCerquetella/agentum/issues/437
> - **Date:** 2026-07-23
>
> ## Problem
>
> When an engineer monitors an autonomous run from its worktree, the gated-run progress topbar can
> vanish as the run crosses a gate or after the engineer returns from another worktree. The missing
> status makes the run appear disconnected precisely when the engineer needs to see its progress.
>
> ## Goal
>
> A workspace operator sees the gated-run progress topbar whenever viewing the worktree owned by a registered harness run.
>
> ## User value
>
> Engineers can trust that the active run's status remains visible while they monitor autonomous work.
>
> ## Acceptance criteria
>
> - [x] The active terminal workspace renders exactly one element labelled `Gated run progress` when its worktree path matches a registered `HarnessStatus`, including while the current feature is `verifying`, `ready_to_test`, `done`, or `blocked`.
> - [x] A harness event that changes the matched run's state renders the updated headline and keeps the progress topbar mounted without an intermediate frame in which it is absent.
> - [x] Switching to another worktree and back renders the owning worktree's progress topbar with its latest status, while a worktree with no matching run renders no progress topbar.
> - [x] Focused Vitest regression checks for gate-state updates and worktree switching return exit code 0.
> - [x] `npm run build --prefix crates/agentum-desktop/ui` returns exit code 0.
>
> ## Scope and non-goals
>
> - **In scope:** persistence and correct active-worktree scoping of the existing gated-run progress topbar across live harness updates and worktree selection changes.
> - **Out of scope:** redesigning the topbar; changing worktree navigation; broad render-performance optimization; changing harness states, gate semantics, backend routes, agent launch, or session streaming; showing a run topbar on non-terminal pages or worktrees that do not own a run.
>
> ## Existing code to reuse
>
> - `crates/agentum-desktop/ui/src/components/Terminal.tsx` already mounts `GatedRunBar` as a root flex strip above the workspace surfaces; retain this single topbar location.
> - `crates/agentum-desktop/ui/src/components/gated-run/GatedRunBar.tsx` already renders the `Gated run progress` region and derives its display from `HarnessStatus`; fix the regression in this existing surface rather than creating another header.
> - `crates/agentum-desktop/ui/src/hooks/useWorktreeHarnessRun.ts` already matches runs by normalized worktree path and refreshes status from harness events; retain its event-driven contract.
> - `crates/agentum-desktop/ui/src/runtime/harness-client.ts` already defines the harness, feature, and event state vocabulary; do not add a parallel UI state model.
> - `crates/agentum-desktop/ui/src/components/gated-run/GatedRunBar.test.tsx` and `GatedRunSurface.test.tsx` provide the focused Vitest patterns to extend.
>
> ## Invariants and overlap
>
> - Harness updates remain push-driven through the existing event stream; this slice adds no polling and does not change the green-gate sequence.
> - The one launch path, YOLO translation, workspace-trust handling, and two-step prompt submission remain unchanged.
> - `ai/specs/023-gated-run-surfacing-and-issue-unlink/spec.md` introduced the gated-run surface; this spec is the narrowly scoped persistence regression for that existing surface, not a competing implementation.
>
> ## Harness wiring
>
> - **Feature entry:** `keep-gated-run-topbar-visible`.
> - **`verify.sh`:** runs the focused Vitest regression checks and the desktop UI build.
> - **`qa.sh`:** observes the topbar through a gate transition, switches away and back, and confirms the latest status remains visible only on the owning worktree.
