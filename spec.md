# Spec 446 — Move Workspace board status through external trackers

- **Status:** Done
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/446

## Problem

When a workspace operator drags a card on the sidebar Workspace board, Agentum changes only its private `workspaceStatus`, so the linked GitHub or Linear item stays in a different status. The operator feels this mismatch immediately after moving a card and can no longer tell which board is authoritative.

## Persona

A workspace operator triaging linked workspaces from the sidebar board expects a card move to update the tracker their team already uses.

## Goal

A workspace operator drags a linked Workspace board card to a lane to move its external tracker item.

## User value

One card move keeps Agentum and the team’s GitHub or Linear workflow aligned.

## Acceptance criteria

- [x] Opening the Workspace board renders `Todo`, `In Progress`, `In Review`, `Ready to Test`, and `Done` lanes and places each linked card in the lane reported by its GitHub or Linear item, without reading `workspaceStatus` as lifecycle truth.
- [x] Dragging one GitHub-linked card to another mapped lane moves the linked GitHub Project item to that status and persists the confirmed canonical phase on the workspace.
- [x] Dragging one Linear-linked card to another mapped lane moves the linked Linear issue to that workflow status and persists the confirmed canonical phase on the workspace.
- [x] Refreshing the Workspace board after an external GitHub or Linear status change renders the card in the externally reported lane.
- [x] An unlinked workspace renders in an explicit `Unlinked` state, and a lane move is blocked with a link-to-tracker action without persisting `workspaceStatus`.
- [x] A rejected, unavailable, or unmapped tracker transition renders an error, leaves the card in its prior lane, and leaves both `trackerPhase` and `workspaceStatus` unchanged.

## Scope and non-goals

- **In scope:** make single-card lane placement and lane moves in the sidebar Workspace board authoritative to the workspace’s existing GitHub or Linear link; show truthful unlinked and failed-move states.
- **Out of scope:** restoring `/api/board` or `board_items`; changing tracker setup or status mappings; adding providers; bulk card moves; changing automated harness/session transitions; deleting legacy `workspaceStatus` metadata.

## Code grounding and reuse

- `crates/agentum-desktop/ui/src/components/sidebar/WorkspaceKanbanDrawer.tsx` and `workspace-kanban-worktree-groups.ts` currently group and move cards through `workspaceStatus`; this is the existing surface to change.
- `crates/agentum-desktop/ui/src/shared/types.ts` and `crates/agentum-server/src/routes/worktrees.rs` already expose `trackerProvider`, `trackerUrl`, and confirmed `trackerPhase` on workspaces.
- `crates/agentum-server/src/task_sink.rs::apply_tracker_transition` already maps canonical phases to GitHub Projects or Linear and emits confirmed or pending outcomes; reuse this write seam.
- `IssueProjectStatusChip.tsx` and `WorktreeCard.tsx` already read linked GitHub and Linear status for workspace cards; reuse those provider reads for board refresh.

## Invariants and overlap

- External tracker success remains required before the board changes lanes; a failed write never invents local progress.
- Spec 027 retired the separate internal task-board API, while this spec changes the surviving sidebar `WorkspaceKanbanDrawer` and does not restore that API.
- `ai/specs/014-live-auto-status/spec.md` explicitly deferred the manual `workspaceStatus` kanban; historical Specs 014/016 target the retired `board_items` mirror, so no existing spec owns this slice.
