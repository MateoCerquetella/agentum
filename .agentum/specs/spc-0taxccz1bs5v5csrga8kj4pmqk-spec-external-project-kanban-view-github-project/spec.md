---
schema: 1
id: SPC-0TAXCCZ1BS5V5CSRGA8KJ4PMQK
revision: 1
title: Spec: External-Project Kanban View (GitHub Projects + Linear)
source: legacy-import:ai/specs/017-external-project-kanban/spec.md@sha256:926da449b2a956112333b6dc0fce1ce35f91e2c22b47dd4240fb2c61000ecc85
---

# Spec: External-Project Kanban View (GitHub Projects + Linear)

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

> # Spec: External-Project Kanban View (GitHub Projects + Linear)
>
> > Builds on the issue-level sync (016a/b + #58, live on develop/staging). Those
> > mirror **issues** into the flat internal board (todo/doing/done). This adds a
> > **kanban view of the external project itself** — a GitHub **Project (v2)** or a
> > **Linear** team/project — rendered with **its own columns**.
> > **PARENT spec** — see the SPLIT in Notes; build **017a first**.
>
> ## Goal
>
> A developer opens a bound GitHub Project (v2) or Linear project inside agentum
> and sees it as a kanban board with the project's own columns and cards.
>
> ---
>
> ## User Value
>
> agentum can already sync individual GitHub/Linear **issues** to its flat
> todo/doing/done board (016/#58). But teams plan in **project boards** — GitHub
> Projects v2 and Linear, each with their own custom columns (Status field options
> / workflow states). There's no way to *see or work those external boards* inside
> agentum; you still switch to the browser to view the real kanban. This surfaces
> the external project board itself — its columns, its cards — so a developer can
> triage and (later) drive the team's actual board without leaving agentum.
>
> ---
>
> ## Requirements
>
> - **Bind a project**: select a GitHub Project (v2) (owner/number) or a Linear
>   team/project as the kanban source (extends 016's binding, which targets a repo
>   for issue-sync — a project is a distinct target).
> - **Fetch the board shape**: read the external project's **columns** (GitHub
>   Project Status options / Linear workflow states) and its **cards** (items),
>   via GraphQL — GitHub Projects v2 and Linear projects are GraphQL, not the REST
>   issue path 016a uses.
> - **Render a kanban view** with the **external project's columns** (dynamic, not
>   the fixed todo/doing/done), each card showing title + state + a deep-link out.
> - **Refresh on demand** ("Sync now" / poll) — self-hosted ⇒ no inbound webhooks.
> - **Fail loud** when the token lacks project scope or the project is unreachable.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] A user can bind a GitHub Project (v2) or a Linear team/project as a kanban
>       source, and the binding persists across restart.
> - [ ] Opening it shows a kanban with **the project's own columns** (e.g. a GitHub
>       Project's Status options, or a Linear team's workflow states), in order.
> - [ ] Each column lists that project's cards under it; each card deep-links to
>       the item in GitHub/Linear.
> - [ ] Refreshing re-fetches and reflects column/card changes made in the tracker
>       (no duplicate columns or cards).
> - [ ] A missing/insufficient token (GitHub Projects need extra scope) or an
>       unreachable project surfaces a clear error, not an empty/silent board.
>
> ---
>
> ## Dependencies
>
> - **016** (server two-way sync) — the binding model + `forge`/`linear` clients +
>   token stores to extend. NOTE: 016a is **REST issues**; this needs **GraphQL**
>   (GitHub `projectsV2`, Linear projects) — net-new transport on both.
> - Existing desktop kanban — `components/board/BoardPage.tsx` +
>   `components/tasks/TaskKanbanBoard.tsx` + `runtime/board-client.ts` (fixed
>   columns today) — the rendering base to generalize to dynamic columns.
> - GitHub token with **Projects** scope; Linear token (already stored).
>
> ---
>
> ## Risks
>
> - **GitHub Projects v2 is GraphQL + extra scope.** Different API from 016a's REST
>   issues, and a classic PAT needs `project`/`read:project` scope (a common
>   failure). *Mitigation: net-new GraphQL client; explicit scope check + clear
>   error.*
> - **Dynamic columns.** The board UI assumes fixed todo/doing/done; project boards
>   have arbitrary, ordered, per-project columns. *Mitigation: generalize the
>   kanban to render columns from the fetched board shape, not a constant.*
> - **"Project" is overloaded.** GitHub Projects v2 ≠ repo issues; Linear projects ≠
>   teams ≠ issues. Binding + fetch must target the project entity specifically.
>   *Mitigation: a distinct project-binding (provider, project-ref) separate from
>   016's repo/issue binding.*
> - **Two-way is hard.** Moving a card across columns means writing the external
>   project's Status field / Linear state. *Mitigation: read-only first (017a/b);
>   drag-to-move is a later slice (017c).*
> - **Scale/scope.** Two providers + GraphQL + a new dynamic-column kanban is beyond
>   one screen — the split below addresses it (PM-gate "fits one screen" fails by
>   design for this parent).
>
> ---
>
> ## Notes
>
> ### Out of scope (this parent / deferred)
> - **Two-way drag-to-move** (write back column changes) → 017c.
> - Custom fields beyond Status/title (assignee, labels, iteration), filters,
>   multiple simultaneous project boards, swimlanes.
> - Replacing the internal board — this is an *additional* view of an external
>   project, alongside the existing internal kanban.
>
> ### Proposed split (vertical slices — build 017a first)
> - **017a — GitHub Project (v2) read-kanban.** FOUNDATIONAL. GraphQL client for
>   `projectsV2` (columns = Status field options + items); a project-binding; a
>   desktop kanban view with the project's dynamic columns. Read-only.
> - **017b — Linear project read-kanban.** Same view for a Linear team/project
>   (workflow states as columns) via GraphQL; reuse the dynamic-column kanban.
> - **017c — Two-way.** Drag a card to another column → write the GitHub Project
>   Status / Linear workflow state back. Define the conflict policy (reuse 016b's).
> - **017d — Deferred.** Custom fields, filters, multiple boards, swimlanes.
>
> Build order: **017a → 017b → 017c → 017d**.
>
> ### Key scoping decisions (confirm at handoff)
> - "GitHub projects" = **GitHub Projects v2** (the kanban product), not repo
>   issues-as-kanban.
> - **Read-first**: 017a/b render the external board; writing back (drag-to-move)
>   is 017c.
> - This is a **new view** layered on 016's binding/client infra, not a rewrite of
>   the internal board.
