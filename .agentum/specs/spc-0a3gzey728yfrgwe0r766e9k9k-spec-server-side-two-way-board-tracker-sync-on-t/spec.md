---
schema: 1
id: SPC-0A3GZEY728YFRGWE0R766E9K9K
revision: 1
title: Spec: Server-Side Two-Way Board ↔ Tracker Sync (on the shipped one-way mirror)
source: legacy-import:ai/specs/016-board-server-two-way-sync/spec.md@sha256:ff3519a68586e1d888eab8214313e431dc480ffcf863cb68c8b6e5a820a37b25
---

# Spec: Server-Side Two-Way Board ↔ Tracker Sync (on the shipped one-way mirror)

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

> # Spec: Server-Side Two-Way Board ↔ Tracker Sync (on the shipped one-way mirror)
>
> > Adds the **server-side two-way half** of board ↔ GitHub/Linear sync **on top of
> > the one-way mirror that already shipped** (#58 "fold Tasks into Board", in
> > v0.19.0): the daemon itself pulls a tracker's issues into the board and pushes
> > card changes (move/edit/close) back out.
> >
> > **Supersedes spec 014.** 014 built the two-way engine on a pre-#58 base; its
> > PR stack (#68/#69/#71, `feat/014a/b/c`) was **closed unmerged** because it
> > collided with #58. The only copy of that work is the local `feat/014d`
> > branch — this spec treats it as a **reference / parts donor to re-port**, not
> > code to merge. **PARENT spec** — see the SPLIT in Notes; build **016a first**.
>
> ## Goal
>
> A developer binds an agentum board to a GitHub repo or Linear team and runs a
> server-side **"Sync now"** that both imports the tracker's issues as cards and
> writes card changes (status, title/body, close) back to the tracker.
>
> ---
>
> ## User Value
>
> Power users already triage work in GitHub/Linear. Today's board can only push
> cards **out one-way on demand from the desktop client** (#58): the *server*
> never talks to a tracker, existing issues can't be imported, and nothing a user
> does on the board (moving a card to done, editing it) ever reaches the tracker.
> This spec closes the loop — one place to import issues, hand them to agents, and
> have status flow back to where the team already looks, driven by the daemon so
> it works for the TUI and any client, not just the desktop fetch path.
>
> ---
>
> ## Requirements
>
> - **Build on #58's data model, don't fork it.** Reuse the shipped
>   `board_items.external_url` / `external_provider` columns and
>   `upsert_board_item_by_external_url`; add only what two-way needs (a stable
>   external **id** + a last-synced marker) via the **next free** migration —
>   `0022` is already taken by `0022_board_external_link.sql`.
> - **Server-side PULL.** The daemon fetches the bound tracker's issues (GitHub
>   REST / Linear GraphQL) and upserts them as cards — create new, update changed,
>   idempotent on re-sync, matched by the durable external reference.
> - **PUSH-back.** Card status ↔ issue state, new local card → new issue,
>   title/body edit → issue update; stamp the external ref back so identity is
>   **stable** (no re-create / ping-pong).
> - **Durable binding.** A persisted board↔tracker binding (GitHub repo or Linear
>   team), re-openable across daemon restart, so "Sync now" knows its target.
> - **One sync contract, no #58 regression.** The shipped client-push path
>   (`POST /api/board/sync` with `{items:[…]}`) must keep working; the server-pull
>   trigger uses a distinct contract/route so the two never clash.
> - **Documented, tested reconcile/conflict policy** for when a card and its issue
>   both changed since the last sync.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] A user can bind a board to a GitHub repo or Linear team, and the binding
>       **persists across a daemon restart**.
> - [ ] A server-side **"Sync now"** imports the bound tracker's open issues as
>       cards carrying their external key + url; re-syncing does **not** duplicate
>       (matched by external ref).
> - [ ] Editing an issue's title or state in the tracker and re-syncing updates the
>       matching card **in place**.
> - [ ] Moving a card to `done` closes the matching issue; creating a local card
>       creates an issue — both keep the **same** external key on the next sync.
> - [ ] The shipped #58 client-push path (`POST /api/board/sync` `{items:[…]}`)
>       still succeeds unchanged — **verified by a regression test**.
> - [ ] When the tracker is unreachable or the token/CLI is missing, sync returns a
>       non-success result with a clear error message and makes **no board changes**
>       (card count and contents unchanged) — verified by a test with a
>       stubbed-unreachable tracker.
> - [ ] When a card and its issue both changed since the last sync, the documented
>       conflict policy is applied and the chosen resolution is **reported in the
>       sync result** (a `conflict`/`conflicts[]` field a test can assert on).
>
> ---
>
> ## Dependencies
>
> - **#58 one-way mirror (FOUNDATION to build on).** Migration
>   `0022_board_external_link.sql`, the `external_url`/`external_provider` columns,
>   `upsert_board_item_by_external_url`, and the desktop board surface
>   (`board-client.ts`, `BoardPage.tsx`, TaskPage "Send to Board"). On
>   develop/staging/main as of v0.19.0.
> - **011 push seam** — `TaskSink` → `gh issue create` and the Linear token store +
>   `teams()` sole-team resolution in `linear.rs` (merged to main via PR #23).
> - **Board API** (`GET/POST/PATCH /api/board`, goals, claim, comments) +
>   **preflight** (`/api/preflight/check` — gh/Linear availability + Linear token).
> - **Reference source to re-port (NOT to merge):** the closed 014a/b/c work on
>   local `feat/014d-board-desktop-ui` — `routes/board_sync.rs`, the `linear.rs`
>   pull/push additions, `forge.rs` `forge_send`, the store external-ref helpers,
>   and the tested `reconcile_status`. Port the logic onto current develop; do not
>   cherry-pick the commits.
>
> ---
>
> ## Risks
>
> - **The three collisions that killed PRs #68/#69/#71 must be designed out, not
>   rediscovered:** (1) **migration** — 014's `0022_board_external_sync` duplicates
>   #58's `0022_board_external_link`; use the next free number (`0023`) and *extend*
>   #58's columns rather than add a parallel schema. (2) **`/api/board/sync` body** —
>   014 used `{binding_id?}`, #58 ships `{items:[…]}`; keep #58's path as-is and put
>   server-pull on a separate route (e.g. `POST /api/board/bindings/{id}/sync`).
>   (3) **`linear.rs` module clash** — 014's board-sync Linear client vs the existing
>   SDD→Linear task-sink client; **merge into the existing `linear.rs`**, one module.
> - **Reconciliation ping-pong** if identity isn't stable. *Mitigation: durable
>   external id + last-synced marker + idempotent upsert — already solved in the
>   reference code's `reconcile_status` (tracker wins on close, reopen → tracker
>   column, else preserve local column; round-trip stable).*
> - **Provider variance.** GitHub state is binary (open/closed); Linear has
>   arbitrary per-team workflow states. *Mitigation: per-provider mapping; reuse
>   014c's `state.type` ↔ column map (backlog/unstarted/started/completed/canceled).*
> - **No inbound webhooks (self-hosted).** Pull is poll/manual; "live" = sync on
>   demand. *Mitigation: explicit "Sync now"; background poll deferred.*
> - **main-checkout WIP hazard.** This repo's working tree routinely holds foreign
>   agent WIP. *Mitigation: build on a fresh branch off develop; never `git add -A`;
>   stage only own hunks.*
> - **Scope.** Server pull + push for two providers + a desktop sync surface is far
>   beyond one screen — the parent fails the PM "fits one screen" gate **by design**
>   (drives the split below; build 016a first).
>
> ---
>
> ## Notes
>
> ### Lineage (why this is not spec 014, and not new-from-scratch)
> - #58 (one-way, **client**-driven mirror) shipped and won: `develop → staging →
>   main`, v0.19.0. It is the foundation.
> - 014a/b/c (two-way, **server**-driven) were built on a pre-#58 base and their PR
>   stack (#68 → develop, #69, #71) was **closed unmerged**. That code survives
>   only on local `feat/014d`. This spec re-delivers its capability *on top of* #58.
>
> ### Out of scope (deferred)
> - **GitLab** — the reference code only half-plumbs it (`forge_send` /
>   `ForgeKind::Gitlab` exist, but `sync_one`/`push_card` branch only github/linear).
> - Background / periodic auto-sync (manual "Sync now" only).
> - Conflict-resolution **UI** (a documented default policy, not a merge UI).
> - Comment / label / assignee / milestone field mapping (status + title/body only).
> - Webhooks / real-time push.
>
> ### Proposed split (vertical slices — build 016a first)
> - **016a — Server-side GitHub PULL + durable binding.** FOUNDATIONAL. Extend
>   #58's columns with a stable external id + synced marker (next-free migration);
>   persist a board↔tracker binding; `POST /api/board/bindings/{id}/sync` imports
>   GitHub issues → cards (idempotent, matched by external ref). One direction, one
>   provider, no #58 regression. **016a explicitly excludes** any push-back, Linear,
>   and any desktop surface — pull + binding + migration only. If it doesn't fit one
>   screen, it's over-scoped.
> - **016b — GitHub PUSH-back + conflict policy.** Card → issue (status map, new
>   card → issue via 011 `GithubSink`, edit → issue update); port + test
>   `reconcile_status`.
> - **016c — Linear parity.** Pull + push for Linear, merged into the existing
>   `linear.rs`; reuse the token store + `teams()` + `state.type` map. *(Note: 016c
>   is two-direction in one slice — the heaviest child; architect may split it
>   pull/push if it overflows one screen.)*
> - **016d — Desktop sync surface.** Bind-tracker UI + per-card external link +
>   "Sync now" on the **existing** `BoardPage` (#58/012 already render the board).
>
> Build order: **016a → 016b → 016c → 016d.**
>
> ### Recommendation
> Treat as a parent; hand **016a** to the architect as the first buildable slice.
> Keep mapping/reconcile as **pure unit-tested functions**; live `gh`/Linear paths
> stay `#[ignore]` / runtime. Do **not** hijack `current_spec` (held on 009a).
