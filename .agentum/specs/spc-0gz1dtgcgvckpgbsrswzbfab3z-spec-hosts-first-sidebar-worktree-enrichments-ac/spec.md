---
schema: 1
id: SPC-0GZ1DTGCGVCKPGBSRSWZBFAB3Z
revision: 1
title: Spec: Hosts-first sidebar — worktree enrichments + active-session card
source: legacy-import:ai/specs/004-sidebar-session-activity/spec.md@sha256:40300cc842e6276e2905b3edafd5dce3b7b69fca8a8716ac665c1b58f8cd93a5
---

# Spec: Hosts-first sidebar — worktree enrichments + active-session card

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

> # Spec: Hosts-first sidebar — worktree enrichments + active-session card
>
> ## Goal
>
> Enrich the worktree leaf rows and the active worktree with live agent activity,
> **re-laying out state that already exists** (no new data plumbing):
> - a **`ctx %` chip** on each worktree leaf (context-window usage),
> - a render-ready, **unwired `PRIMARY`** slot on the leaf,
> - an **active-session card** under the currently-selected worktree showing its
>   last agent message + last tool call.
>
> Final enrichment layer on top of [[002-sidebar-host-grouping]] /
> [[003-sidebar-host-metadata]].
>
> ---
>
> ## User Value
>
> **In one line:** see, without opening a session, how close each agent is to its
> context limit and what the active one just did — triage at a glance.
>
> - **Who:** the user running several agents at once who needs to spot a
>   context-pressured or recently-active session fast.
> - **Why now:** the data (`agentStatusByPaneKey`: `lastAssistantMessage`,
>   `toolName`, `toolInput`, `contextUsagePercent`) is already in the store and on
>   the mockup; it's purely a re-layout — cheap, high signal.
> - **Cost of doing nothing:** the sidebar shows status dots but not *why*; users
>   open sessions just to check context/recent activity.
>
> ---
>
> ## Requirements
>
> - **`useLatestAgentActivity(worktreeId)`** selector — scans
>   `agentStatusByPaneKey` entries belonging to the worktree (reuse the filtering
>   in `hooks/useWorktreeAgentRows.ts`), picks the most-recently-updated, returns
>   `{ lastAssistantMessage, toolName, toolInput, contextUsagePercent }`. Single
>   source for both the card and the leaf's ctx% chip.
> - **ctx% chip** on each worktree leaf — muted `ctx N%`, where session-level ctx =
>   aggregate (max) of per-agent `contextUsagePercent`; omitted when no agent
>   reports a value.
> - **`PRIMARY` slot** — render-ready pill on the leaf, **unwired** (renders only
>   when a future flag is set; no meaning assigned in v1).
> - **`SessionActivityCard.tsx`** (new) — renders **only** for `activeWorktreeId`,
>   slotted under that leaf: last agent message (truncate, ≤ existing ~8KB field)
>   + last tool call (`toolName` ≤60, `toolInput` ≤160), with a timestamp.
> - **No new endpoints / no new plumbing** — all four fields already exist in
>   `agent-status-types.ts`; this spec only reads + lays them out.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] **ctx% chip** — a worktree leaf with agent context data **shows** `ctx N%`
>       (N = aggregated max); a leaf with none **omits** the chip (unit-tested
>       aggregation: multiple agents → correct value; none → undefined).
> - [ ] **Latest activity** — given several `agentStatusByPaneKey` entries for one
>       worktree, `useLatestAgentActivity` **returns** the most-recent entry's
>       fields; **returns empty** when there are none (unit-tested).
> - [ ] **Card scope** — `SessionActivityCard` **renders only** for
>       `activeWorktreeId`, under that leaf, and **not** for other worktrees.
> - [ ] **Truncation** — long message/tool strings **truncate** cleanly (render
>       test); no layout overflow.
> - [ ] **PRIMARY** — the slot **renders** only when its (future) flag is set; off
>       by default in v1.
>
> ---
>
> ## Dependencies
>
> - [[002-sidebar-host-grouping]] (host→repo→worktree rows + leaf rows to enrich).
> - Existing state, unchanged: `agentStatusByPaneKey` (`slices/agent-status.ts`),
>   `activeWorktreeId` (`slices/worktree-helpers.ts`), the filtering in
>   `hooks/useWorktreeAgentRows.ts`.
>
> ---
>
> ## Risks
>
> - **Re-render churn.** Card + ctx% read frequently-updating agent status; must be
>   memoized/selector-scoped so they don't re-render the whole virtualized list on
>   every agent tick.
> - **ctx aggregation semantics.** "Session ctx" from multiple agents — define as
>   max (worst-case headroom); document so it isn't read as an average.
> - **Card height in a virtualized list.** The active leaf's card changes row
>   height — the virtualizer must measure/repaint it without scroll jump.
>
> ---
>
> ## Notes
>
> **Out of scope:** wiring `PRIMARY` to a real meaning (design-system slot only);
> host grouping/header (002); OS/arch line (003). Design ref:
> `docs/superpowers/specs/2026-06-05-desktop-hosts-sidebar-design.md` §4.2–4.3,
> §5, §8 (Resolved decisions: card = re-layout, no plumbing; PRIMARY unwired;
> badge included).
