---
schema: 1
id: SPC-1T07R8QZS34D30FTV96C95FA9S
revision: 1
title: Spec: Desktop Board-Goals View
source: legacy-import:ai/specs/012-desktop-board-goals/spec.md@sha256:7c92742be94f855c8da1eddd322fafea4bf8f158ff302384d1aaafe9962cb000
---

# Spec: Desktop Board-Goals View

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

> # Spec: Desktop Board-Goals View
>
> > Unblocks the spec 011 desktop trigger: gives the "Plan harness from goal"
> > button a home. Tracking: GitHub issue #25 (follow-up to #19).
>
> ## Goal
>
> A desktop user creates a goal in natural language, watches it decompose into
> feature cards, and clicks "Plan harness" to push those features into the
> harness — all from a board view inside the app.
>
> ---
>
> ## User Value
>
> The chat-to-features pipeline (spec 011) is complete in the backend but
> **unreachable from the desktop app**: there is no UI that renders board goals
> (`board_items` with `lbl=goal`) or their child cards — that flow lives only in
> the TUI / board API. This spec surfaces it, so a desktop user can go from "an
> idea" to "a planned harness backlog" without leaving the app or dropping to a
> terminal.
>
> ---
>
> ## Requirements
>
> - A desktop **board-goals view**: list goals (`board_items` `lbl=goal`) and,
>   per goal, its child feature cards (`parent_goal_id`), read from the existing
>   board API (`GET /api/board`).
> - **Create a goal** from a natural-language description (`POST /api/board/goals`)
>   — the existing planner decomposes it into child cards (no new backend).
> - A per-goal **"Plan harness"** action calling the existing
>   `planGoalHarness(goalId)` client (`POST /api/board/goals/{id}/harness-plan`),
>   showing the resulting provider + feature count, with loading/error states.
> - After planning, a path to the **harness Run** (open the existing Harness view
>   for that workdir) — Run stays the human action.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] The board-goals view lists existing goals and each goal's child cards.
> - [ ] Submitting a natural-language goal creates it and the new goal appears in
>       the view (child cards appear as the planner produces them).
> - [ ] Each goal shows a "Plan harness" button; clicking it calls the endpoint
>       and displays the returned `provider` + `feature_count` (or a clear error).
> - [ ] After a successful plan, the user can open the Harness view for that
>       goal's workdir and Run it.
> - [ ] A goal with no child cards yet shows the backend's "let the planner
>       decompose it first" message rather than a silent failure.
> - [ ] `npm run build --prefix crates/agentum-desktop/ui` (Vite) succeeds.
>
> ---
>
> ## Dependencies
>
> - **011** — `planGoalHarness()` client (already in `harness-client.ts`) and the
>   `POST /api/board/goals/{id}/harness-plan` endpoint.
> - Existing board API: `GET /api/board`, `POST /api/board/goals`.
> - Existing desktop Harness view (`HarnessEngine.tsx` / `ChatPage.tsx`) for the
>   Run hand-off.
>
> ---
>
> ## Risks
>
> - **Async planner timing.** Child cards are created asynchronously by the
>   planner agent after the goal POST; the view must refresh (poll or the global
>   event bus `goal.*` / `board.*` events) rather than assume cards exist
>   immediately. Mitigation: subscribe to the existing events bus / refresh on
>   `goal.harness.planned`.
> - **No board client in the desktop UI today.** A small `board-client.ts` must be
>   added (mirrors `harness-client.ts`); keep it thin over the existing routes,
>   not a reimplementation.
> - **Surface placement / navigation.** Adding a new `activeView` (e.g.
>   `'goals'`) touches the app router + sidebar; keep the change small and match
>   the existing view-switch pattern.
> - **Frontend-only verification.** Needs the npm/Vite build env; known repo
>   gotchas (bare `tsc` can't resolve `shared/*`; some vitest addons fail) are
>   pre-existing noise, not this spec's concern.
>
> ---
>
> ## Notes
>
> ### Out of scope
> - New backend work — the board + goals + harness-plan APIs already exist.
> - Editing/My deleting goals or drag-reordering (read + create + plan only in v1).
> - The TUI (already has the board flow).
>
> ### Likely split (if the PM gate flags "fits one screen")
> - **012a** — read-only board-goals view (list goals + child cards) + thin
>   `board-client.ts`.
> - **012b** — create-goal input (NL → planner) wired into the view.
> - **012c** — "Plan harness" button + result surface + Harness Run hand-off.
>
> Build order: 012a → 012b → 012c. 012c is the slice that finally lights up the
> spec 011 button.
