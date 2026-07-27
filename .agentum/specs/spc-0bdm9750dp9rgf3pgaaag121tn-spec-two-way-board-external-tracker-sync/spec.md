---
schema: 1
id: SPC-0BDM9750DP9RGF3PGAAAG121TN
revision: 1
title: Spec: Two-Way Board ↔ External Tracker Sync
source: legacy-import:ai/specs/014-board-tracker-sync/spec.md@sha256:032be6e47323c8ce844532e309390c96f200f0ff911e27b1a3ca78eb981dbcf7
---

# Spec: Two-Way Board ↔ External Tracker Sync

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

> # Spec: Two-Way Board ↔ External Tracker Sync
>
> > ⚠️ **SUPERSEDED by [spec 016](../016-board-server-two-way-sync/spec.md) (2026-06-22).**
> > This spec's premise ("no way to see tracker issues on the board") was overtaken
> > by **#58** (one-way client mirror), which shipped to main in v0.19.0. The 014a/b/c
> > build (PRs #68/#69/#71) was **closed unmerged** — it collided with #58 and now
> > lives only on local `feat/014d`. Spec 016 re-delivers the two-way half *on top of*
> > #58. Kept here as the historical record of the abandoned approach.
>
> > Folds GitHub Issues / Linear into agentum's board as a **two-way mirror**:
> > external issues appear as board cards, and board changes write back to the
> > tracker. This is the "fold GitHub/Linear Tasks into board" piece deferred
> > from the #48 board redesign, and the bidirectional half deferred from spec
> > 011 (chat-to-features), which only pushes one-way on card create.
> > **PARENT spec** — see the SPLIT in Notes; build **014a first**.
>
> ## Goal
>
> A developer binds an agentum board to a GitHub or Linear project and works the
> board as the live two-way mirror of that tracker — external issues show up as
> cards, and creating/moving/closing cards updates the tracker.
>
> ---
>
> ## User Value
>
> Power users run agents across external projects whose work already lives in
> GitHub Issues or Linear. Today agentum's board is an isolated local kanban
> (TUI/API only) and spec 011 only *pushes* new cards out one-way on create —
> there is no way to see existing tracker issues on the board or keep the two in
> step. This spec makes the board usable *with* the user's real projects: one
> place to triage issues, hand them to agents, and have status flow back to where
> the team already looks — without leaving agentum or double-entering tasks.
>
> ---
>
> ## Requirements
>
> - **Bind** a board scope to an external tracker target (a GitHub repo, or a
>   Linear team/project), persisted server-side and re-openable across restart.
> - **Pull (import):** fetch the tracker's issues and upsert them as board cards —
>   create new, update changed, idempotent on re-sync — matched by a durable
>   per-card external reference (provider + external id + url).
> - **Push (write-back):** propagate board changes to the tracker — card status ↔
>   issue state, new local card → new issue, title/body edits → issue edit.
> - **Sync trigger:** a manual "Sync now" action that runs both directions
>   (self-hosted ⇒ no inbound webhooks, so pull is poll-based / on-demand).
> - **Status mapping:** an explicit, bidirectional mapping between board columns
>   (todo/doing/done) and tracker states, with a documented conflict policy for
>   when both sides changed since the last sync.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] A user can bind a board to a GitHub repo or Linear team, and the binding
>       persists across a daemon restart.
> - [ ] After a sync, every open issue in the bound tracker appears as a board
>       card carrying its external key + url; re-syncing does **not** duplicate
>       cards.
> - [ ] Editing an issue's title or state in the tracker and re-syncing updates
>       the matching card in place (no duplicate created).
> - [ ] Moving a card to `done` closes the matching issue; creating a local card
>       creates an issue — both reflected on the next sync.
> - [ ] A card created locally and pushed keeps the **same** external key on the
>       next sync (stable identity — no re-create / ping-pong loop).
> - [ ] When the tracker is unreachable or the CLI/token is missing, sync **fails
>       loudly** with a clear message — no silent partial state.
> - [ ] A documented conflict policy is applied (and visible in the sync result)
>       when a card and its issue both changed since the last sync.
>
> ---
>
> ## Dependencies
>
> - **011 (chat-to-features)** — the external **write/push seam** (`TaskSink` →
>   `gh issue create` / Linear `issueCreate`) and provider selection
>   (`pick_provider`). Per project memory this currently lives on **staging**, not
>   the `main` checkout — the architect must confirm the target branch and
>   **reuse** it rather than reimplement the push half.
> - **Board API** — `GET/POST/PATCH /api/board`, `/api/board/goals`,
>   claim/comments (present on `main`).
> - **preflight** (`/api/preflight/check`) — gh / Linear availability + the Linear
>   token store.
> - **#48 desktop board redesign** — only relevant if a desktop surface is in
>   scope (see split 014d): the `board_items` kanban has **no desktop view today**
>   (TUI/API-only).
>
> ---
>
> ## Risks
>
> - **No server-side binding or external-ref today.** `board_items` has no
>   provider/external-id column and there is no persisted projects/repos store —
>   both are net-new (migration + store methods). *Mitigation: 014a lays this
>   foundation first.*
> - **Two-way reconciliation is the hard part.** Matching, change detection, and
>   conflict resolution can loop (re-create / ping-pong) if identity isn't stable.
>   *Mitigation: durable external_ref + last-synced marker; idempotent upsert;
>   tracker-wins default; heavy unit coverage of the pure mapping logic.*
> - **No inbound webhooks (self-hosted constraint).** Pull is poll/manual; "live"
>   means sync-on-demand, not push. *Mitigation: explicit "Sync now"; background
>   polling deferred to 014e.*
> - **Provider variance.** GitHub state is binary (open/closed); Linear has
>   arbitrary per-team workflow states. A single status map will mis-map.
>   *Mitigation: per-provider mapping with sensible defaults, documented.*
> - **Rate limits / latency.** `gh` CLI and Linear GraphQL are network calls;
>   large repos are slow. *Mitigation: default to open issues, paginate, surface
>   progress, keep sync off the hot path.*
> - **Scope.** Full two-way for two providers + a new desktop surface is far
>   beyond one screen — addressed by the split below (the PM-gate "fits one
>   screen" check fails for this parent **by design**).
>
> ---
>
> ## Notes
>
> ### Out of scope (this parent / deferred to 014e)
> - Background / periodic auto-sync (manual "Sync now" only in 014a–d).
> - GitLab and other providers.
> - Comment, label, assignee, milestone, attachment field mapping
>   (status + title/body only).
> - A conflict-resolution **UI** (a documented default policy, not a merge UI).
> - Webhooks / real-time push.
>
> ### Proposed split (vertical slices — build 014a first)
> - **014a — Mapping foundation + GitHub PULL (import).** FOUNDATIONAL. Net-new:
>   durable per-card `external_ref` (provider, external id, url, synced marker) +
>   a persisted board↔tracker binding + a sync endpoint that imports GitHub issues
>   → cards (create/update, idempotent, matched by external_ref). One direction,
>   one provider. Delivers: board mirrors a repo. Reuses `gh` detection +
>   preflight.
> - **014b — GitHub PUSH-back.** Board → tracker: status → issue open/closed
>   (status map), new card → issue (reuse 011 `GithubSink`), title/body edit →
>   `gh issue edit`. GitHub now truly two-way; defines + tests the conflict policy.
> - **014c — Linear parity.** Same two-way loop for Linear (pull via `issues`
>   GraphQL query, push via `issueCreate`/`issueUpdate`, workflow-state map).
>   Reuses 011 `linear.rs` (token store + `teams()` sole-team resolution).
> - **014d — Desktop board + sync surface.** A desktop board view reading
>   `/api/board` (none exists today) showing the bound tracker, per-card external
>   link, sync status, a "Sync now" action, and the bind-tracker UI. May split
>   further (read-view → sync controls).
> - **014e — Deferred.** Background auto-sync, conflict-resolution UI, GitLab,
>   extra field mappings, webhooks.
>
> Build order: **014a → 014b → 014c** (after 014b proves the two-way pattern)
> **→ 014d → 014e**.
>
> ### Recommendation
> Treat this as a parent. Hand **014a** to the architect as the first buildable
> slice. Keep `current_spec` discipline — do **not** hijack an active developer
> spec (009a). Live `gh`/Linear paths stay runtime / `#[ignore]`; the
> mapping + reconciliation logic should be pure functions, unit-tested hard.
