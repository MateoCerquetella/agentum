---
schema: 1
id: SPC-03W9P04GJ08A33RRTY1CXF0CM7
revision: 1
title: Issue hover card shows the bound GitHub Project's Status
source: legacy-import:ai/specs/018-issue-hover-project-status-chip/spec.md@sha256:5238c6297d90ba185320abba7be10d4ee26bb371dd924b887b91eb33988f4ccf
---

# Issue hover card shows the bound GitHub Project's Status

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

> # Spec 018 — Issue hover card shows the bound GitHub Project's Status
>
> - **Number:** 018
> - **Status:** Done       <!-- Draft | PM | Architect | In progress | Done -->  (Reviewer SIGN-OFF 2026-07-14, `review.md`, 0 blockers. Built as one slice; UI build + all new vitest + tsc + fmt green; Rust unit gate CI-deferred (no local webkitgtk). Merged to develop; #365 stays open until it reaches main. Downstream: qa.sh live legs at staging.)
> - **Surface:** `crates/agentum-desktop/ui` (sidebar hover card) + `crates/agentum-desktop/src/commands/gh_projects.rs` (one read command)
> - **Author:** Claude (from Mateo's ask, GitHub issue #365)
> - **Date:** 2026-07-14
> - **Tracker:** https://github.com/MateoCerquetella/agentum/issues/365
>
> > Lineage: split out of spec 016 (F2 rider, AC 8–10) at its PM gate — bundling
> > it with the SDD-loop slice violated the one-slice rule. The harness-side
> > stub is `.agentum-harness/specs/358b-issue-hover-project-status-chip/spec.md`;
> > this document is its SDD spec and PM gate. Code citations are against
> > `origin/develop` @ `d31314b3` (this worktree was fast-forwarded to it on
> > 2026-07-14 — the 358b stub's "v0.57.0-era checkout" warning no longer applies
> > here, but re-verify lines before editing if develop moves again).
>
> ## Problem
>
> Mateo hovers an issue badge on a worktree card in the desktop sidebar to check
> where a ticket stands. The hover card shows only the issue's open/closed state
> and labels — the issue's actual GitHub Project column (Status: Todo / In
> Progress / …) is invisible even when the repo has a Projects v2 binding, so he
> has to open GitHub to see the board position.
>
> ## Goal
>
> The sidebar issue hover card renders the bound GitHub Project's Status option
> name for the linked issue.
>
> ## Users / personas
>
> - **Mateo (self-hosting engineer) scanning the sidebar**: hovers a workspace's
>   issue badge and wants to see where the ticket sits on the configured GitHub
>   Project board without leaving the app.
>
> ## Acceptance criteria
>
> 1. When the worktree's repo has a Projects v2 binding
>    (`GET /api/github/project-binding` returns non-null) and the hover card
>    (`WorktreeCardDetailsHover`, `WorktreeCardMeta.tsx:218`) opens for a linked
>    GitHub issue, the card **renders** a Project-status chip with the issue's
>    current Status option name (e.g. "In Progress") — visually distinct from
>    the open/closed `IssueStateBadge` (`WorktreeCardMeta.tsx:316`) and the
>    internal `TrackerPhaseChip` (`WorktreeCardMeta.tsx:320`).
> 2. No binding, issue not on the bound project, or fetch error → the card
>    **renders** no chip (silent absence: no error state, no layout shift, card
>    otherwise byte-identical to today).
> 3. The status is **fetched** lazily on first card open and **cached** per
>    issue for the app session: a second hover on the same issue **triggers** no
>    new fetch, and a card never hovered **triggers** no fetch at all.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** the chip; one single-issue Status read; the lazy fetch-on-open hook
>   + session-scoped per-issue cache; reusing the existing binding read.
> - **Out:**
>   - No Status *editing* from the hover card (the Project Hub board already
>     does that via `gh_update_project_item_field`).
>   - No Linear equivalent — GitHub Projects only.
>   - No live refresh / polling / event subscription for the chip — it is a
>     snapshot at card-open; `TrackerPhaseChip` already streams live phase.
>   - No changes to `IssueStateBadge`, `TrackerPhaseChip`, or the Project Hub
>     board/table surfaces.
>   - No new server route (the recommended read path is desktop-side; see open
>     question 1).
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `WorktreeCardDetailsHover` (`crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx:218`)
>   — the hover card; it already holds an `open` state via
>   `<HoverCard open onOpenChange…>` (:254), which is the lazy-fetch trigger.
>   The chip slots into the existing badges row (:314–:321), which already
>   renders `IssueStateBadge` + `TrackerPhaseChip` and receives `worktreeId` /
>   `trackerPhase` props (:58–:59).
> - Binding read: server `routes/github_projects.rs::get_binding`
>   (`crates/agentum-server/src/routes/github_projects.rs:273`,
>   `GET /api/github/project-binding?workdir&slug&repoId`, keyed by slug via
>   `binding_for_slug`) + its UI client `getProjectBinding`
>   (`crates/agentum-desktop/ui/src/runtime/github-projects-client.ts:144`).
>   Host-aware: pass `repoId` so SSH repos resolve to the same binding (spec
>   020 wire) — do not re-derive slugs client-side.
> - Projects v2 GraphQL plumbing (`crates/agentum-desktop/src/commands/gh_projects.rs`):
>   the `graphql()` runner (:136), `classify_graphql_errors` (:66) /
>   `classify_stderr` (:98), and `gh_get_project_view_table` (:766) as the
>   pattern for the new single-issue read. The `#[cfg(test)]` mapping-test
>   style in the same file is the unit-test template.
> - Typed Tauri client seam: `ui/src/tauri/gh.ts` (+ `tauri/contract.ts`,
>   command registration in `crates/agentum-desktop/src/lib.rs`) — add the new
>   command there, matching the existing `gh_*` naming.
> - Badge precedent: `IssueStateBadge` / `LinearStateBadge`
>   (`ui/src/components/sidebar/WorktreeCardMetadataStatusBadges.tsx`) — the
>   chip should look like a sibling of these, not a new visual system.
> - Test seam: `WorktreeCardMeta.test.tsx` (same dir) already exercises the
>   hover card — extend it for chip presence/absence.
>
> ### Build new
>
> - One GraphQL read: issue → `projectItems` → item whose project matches the
>   bound `(owner, number)` → `fieldValueByName("Status")` single-select option
>   name. Recommended shape: a desktop Tauri command (working name
>   `gh_issue_project_status`) beside `gh_get_project_view_table`, returning
>   the option name or null; all error envelopes map to absence in the UI.
> - A lazy fetch-on-open hook (e.g. `useIssueProjectStatus`) keyed off the
>   hover card's `open` state, with an app-session cache per
>   `(slug, issueNumber)` — plus a session-cached binding lookup per repo so
>   repeated hovers don't re-hit `/api/github/project-binding` either.
> - The chip component itself (a few lines beside `TrackerPhaseChip` in the
>   badges row).
>
> ## Risks & invariants
>
> - **GitHub rate limits:** fetch only on card open + session cache (AC 3);
>   never poll. A flaky `gh` must degrade to silent absence (AC 2), never a
>   broken hover card.
> - **Crate boundaries:** `agentum-server` stays API-only and untouched — the
>   binding read reuses the existing route; the GraphQL read lives desktop-side
>   where `gh` auth already lives (same placement as every `gh_projects.rs`
>   command).
> - **SSH repos (spec 020):** thread `repoId` into `getProjectBinding` so a
>   bound SSH repo shows the chip too. The Status GraphQL read itself is
>   slug-based and runs on local `gh` auth — if the local token can't see the
>   project, that's a fetch error → silent absence (acceptable).
> - **No regression when unbound:** the common case (repo with no binding) must
>   render the card exactly as today — one cached binding miss, no chip, no
>   spinner.
> - **Renderer fail-closed:** any unexpected payload shape → absence, never a
>   crash in the badges row (the card wraps the issue section, a throw would
>   take the whole hover down).
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** one feature — "Issue hover card shows the
>   bound GitHub Project's Status for the linked issue".
> - **`verify.sh` asserts:** `cargo test -p agentum-desktop --lib` green
>   (mapping tests for the new command in `gh_projects.rs`'s tests mod);
>   `cargo fmt --check`; UI build green (`bun run build` in
>   `crates/agentum-desktop/ui`); targeted vitest green (`bunx vitest run` on
>   the fetch-cache model + `WorktreeCardMeta.test.tsx`) — the full vitest
>   suite has a known pre-existing failure baseline, so the gate pins the
>   targeted files.
> - **`qa.sh` asserts:** (browser QA) hovering an issue badge on a
>   Project-bound repo shows the Status chip with the board column name;
>   hovering on an unbound repo shows no chip; a second hover on the same issue
>   issues no new fetch (assert via request log/instrumentation).
>
> ## Open questions — RESOLVED by the architect (`architecture.md` §1)
>
> 1. ~~Read path~~ → **D1: desktop Tauri command** `gh_issue_project_status`
>    beside `gh_get_project_view_table`. A server route buys nothing — an
>    unreadable/absent status is already a silent no-chip (AC 2).
> 2. ~~Binding source~~ → **D2: fresh `getProjectBinding`, cached per slug** for
>    the app session. Project Hub store reuse is unreliable (only populated once
>    that page opens); the hint fast-path makes the fetch zero-git-I/O.
