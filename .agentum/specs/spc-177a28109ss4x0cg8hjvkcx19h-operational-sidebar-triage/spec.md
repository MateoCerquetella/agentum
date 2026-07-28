---
schema: 1
id: SPC-177A28109SS4X0CG8HJVKCX19H
revision: 1
title: Operational sidebar triage
source: legacy-import:ai/specs/025-operational-sidebar-triage/spec.md@sha256:8e67c77c49d5781395ddee19e76bb43eef2d5c9cf4ee05972f988c61756a0dc6
---

# Operational sidebar triage

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

> # Spec 025 — Operational sidebar triage
>
> - **Number:** 025
> - **Status:** Done — Reviewer signed off; ready to ship
> - **Surface:** `crates/agentum-desktop/ui`
> - **Author:** Codex
> - **Date:** 2026-07-22
>
> ## Problem
>
> An engineer supervising agents across several projects must scan a long, project-oriented
> workspace tree to discover which agent needs a response, which agents are still running, and
> which workspaces are quiet. Search, project scoping, agent state, and recency are distributed
> across separate controls and card details, so the operator spends time hunting instead of
> responding to the next actionable workspace.
>
> ## Goal
>
> The desktop sidebar gives an operator one compact, searchable operational queue ordered by
> Needs You, Active, and Settled state.
>
> ## Users / personas
>
> - **Primary:** an engineer supervising multiple local or SSH-hosted coding agents who needs to
>   find the next workspace requiring intervention without opening each terminal.
> - **Moment:** while agents are running concurrently and the engineer is repeatedly scanning,
>   filtering, and reopening workspaces from the left sidebar.
>
> ## Acceptance criteria
>
> 1. When the operational grouping is selected, the sidebar renders the complete filtered
>    workspace set in exactly three ordered sections: **Needs You**, **Active**, and **Settled**;
>    each section header displays the count of all matching workspaces in that section.
> 2. A workspace renders in **Needs You** when any fresh agent signal is blocked, waiting, or
>    awaiting input; otherwise it renders in **Active** when an agent is working or has reached a
>    fresh ready/done state; all remaining workspaces render in **Settled**. A workspace appears
>    in one section only, and the more urgent state wins when panes disagree.
> 3. The top of the workspace area renders a text search with its configured keyboard shortcut,
>    an **All** project control, visible project filter controls, an overflow control for projects
>    that do not fit, and the existing new-workspace action. Search filters by workspace display
>    name, branch, project name, and visible agent label; project controls compose with search,
>    and clearing both restores the full queue.
> 4. Needs You and Active workspaces render as information-rich cards containing the project
>    name, truthful operational label, workspace display name, branch, agent label when known,
>    and a relative state/activity age. Missing optional metadata is omitted without placeholder
>    noise, and long values truncate without widening or horizontally scrolling the sidebar.
> 5. Settled workspaces render as compact one-line rows ordered by most recent activity. The
>    section initially reveals at most three rows, renders **Show N more** only when rows remain,
>    and expands/collapses in place without changing the search, project filters, active
>    workspace, or section counts.
> 6. Clicking, keyboard-activating, context-clicking, dragging, selecting, or revealing a
>    workspace from the V2 presentation invokes the same existing workspace behavior as the
>    current list; the active workspace remains visually distinct in both rich and compact rows.
> 7. Operational grouping is the default only when no grouping preference has been persisted.
>    Existing persisted grouping choices remain unchanged, and the existing alternate grouping,
>    sorting, filtering, board, project, host, and workspace-management controls remain reachable.
> 8. At the supported 220–500 px sidebar widths, both light and dark themes preserve readable
>    hierarchy, visible focus, and WCAG AA text contrast; keyboard users can reach search, every
>    project filter, each section control, and every visible workspace in logical top-to-bottom
>    order.
> 9. Focused unit/render tests pass and `npm run build --prefix
>    crates/agentum-desktop/ui` completes successfully.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** a desktop-only operational grouping and its default for unconfigured installs;
>   inline workspace search; project quick filters with overflow; authoritative triage
>   classification; rich Needs You/Active cards; compact, progressively disclosed Settled rows;
>   responsive, themed, accessible interaction states.
> - **Out:** backend routes, SQLite or persisted worktree-schema changes; changing watchdog or
>   agent-status detection; removing existing grouping/sort modes; redesigning Mission Control,
>   Projects, the workspace board, right sidebar, dialogs, or terminal content; new notification
>   rules; changing project colors; changing what clicking or dragging a workspace does.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `Sidebar` (`components/sidebar/index.tsx:34`) owns the 220–500 px resizable shell and fixed vs
>   scrollable regions; retain that boundary and resize behavior.
> - `SidebarNav` and `SidebarHeader` (`components/sidebar/SidebarNav.tsx:84`,
>   `components/sidebar/SidebarHeader.tsx:10`) own primary navigation, workspace creation, board,
>   filter, and options entry points; reorganize or compose these controls rather than duplicating
>   their actions.
> - `buildAttentionByWorktree` and `SmartClass` (`components/sidebar/smart-attention.ts:30,301`)
>   already resolve multi-pane urgency with fresh hook data and title fallback; adapt this result
>   into the three presentation sections instead of inventing a second status detector.
> - `selectWorktreeAgentActivitySummary` (`components/sidebar/worktree-agent-activity-summary.ts:45`)
>   already folds live, retained, and watchdog awaiting-input signals per workspace; reuse its
>   precedence for truthful labels.
> - `latestFromEntries` (`components/sidebar/worktree-latest-activity.ts:17`) selects the newest
>   agent metadata, while `Worktree` (`shared/types.ts:238`) already supplies display name,
>   branch, project binding, and activity timestamps.
> - `WorktreeCard` (`components/sidebar/WorktreeCard.tsx:100`) owns activation, context menu,
>   drag/drop, rename, active styling, and workspace quick actions. Extract/share presentation
>   primitives or add a deliberate variant; do not fork those behaviors.
> - `buildRows` and `GroupHeaderRow` (`components/sidebar/worktree-list-groups.ts:177,595`) plus
>   `WorktreeList`'s existing virtualizer remain the authoritative flat-list path for large
>   workspace sets.
>
> ### Build new
>
> - A pure operational-sidebar view model that composes text/project filtering, maps existing
>   attention signals to the three mutually exclusive sections, sorts within sections, and
>   calculates full counts plus settled disclosure state.
> - A compact control header for inline search, project quick filters/overflow, shortcut hint,
>   and new-workspace action using existing buttons, menus, tokens, and actions.
> - Rich operational-card and compact settled-row presentation variants that share the existing
>   `WorktreeCard` interaction contract.
> - Focused tests for classification precedence, composed filtering, counts, ordering, disclosure,
>   keyboard semantics, action parity, and narrow-width truncation contracts.
>
> ## Risks & invariants
>
> - **Truth beats decoration:** fresh explicit hook/watchdog state remains authoritative; stale
>   status must not pin a workspace in Needs You or Active. Title heuristics remain fallback only.
> - **One workspace, one section:** classification must be deterministic across multiple panes and
>   use urgent-state precedence so duplicate or contradictory rows cannot appear.
> - **Virtualization stays intact:** filtering, variable-height rich cards, disclosure, and live
>   state transitions must preserve scroll/reveal anchoring and avoid remount churn or list jumps.
> - **Interaction parity:** the redesign is a presentation/view-model change, not a second
>   activation, drag, selection, context-menu, or deletion implementation.
> - **No preference surprise:** only absent grouping state receives the operational default;
>   explicit persisted choices are never overwritten.
> - **Push stays push-based:** the UI derives from existing store/event updates and introduces no
>   status polling or backend launch-path changes.
> - **Remote parity:** local and SSH-hosted workspaces use the same classification and controls;
>   disconnected styling and reconnect affordances remain truthful.
>
> ## Harness wiring (the gate)
>
> - **`feature_list.json` entry:** `sidebar-operational-triage` — deliver the operational view
>   model, header controls, rich/compact rows, progressive disclosure, and interaction parity as
>   one user-visible slice.
> - **`verify.sh` asserts:** focused Vitest suites cover urgent-state precedence, stale-state
>   fallback, section exclusivity/counts, composed search/project filtering, settled ordering and
>   disclosure, persisted-grouping default behavior, keyboard semantics, and shared workspace
>   actions; then the desktop UI production build exits 0.
> - **`qa.sh` asserts:** in the real desktop browser surface, seed at least one Needs You, Working,
>   Ready, and four Settled workspaces across two projects; verify counts and order, search plus
>   project filter composition, overflow project selection, Show N more/collapse stability,
>   activation/context-menu/drag parity, keyboard traversal, 220 px and 500 px layouts, and light
>   and dark theme screenshots.
>
> ## Open questions
>
> - None. The V2 reference establishes the hierarchy; existing Agentum status, interaction, and
>   preference primitives define the implementation truth.
