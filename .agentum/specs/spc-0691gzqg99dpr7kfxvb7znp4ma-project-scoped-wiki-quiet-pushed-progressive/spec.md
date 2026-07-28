---
schema: 1
id: SPC-0691GZQG99DPR7KFXVB7ZNP4MA
revision: 1
title: Project-scoped Wiki (quiet, pushed, progressive)
source: legacy-import:ai/specs/009-wiki-project-scoped/spec.md@sha256:0e4a1872e255fc91b24a26cda3c50e45346822bc7b6ad5f6185f297f8e28b82c
---

# Project-scoped Wiki (quiet, pushed, progressive)

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

> # Spec 009 — Project-scoped Wiki (quiet, pushed, progressive)
>
> - **Number:** 009
> - **Status:** Done              <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-desktop/ui` (sidebar, WikiPage, ProjectHub) + `crates/agentum-server` (`routes/wiki.rs`, `routes/fs.rs`, events)
> - **Author:** Mateo Cerquetella (drafted with Claude)
> - **Date:** 2026-07-06
> - **Base:** `origin/develop` @ `388eaa66` (v0.58.3) — citations verified at this commit
>
> ## Problem
>
> Opening the global **Wiki** view sweeps *every* registered repo — one
> `GET /api/wiki` per repo, each shelling `git remote get-url origin` inside that
> repo's directory. With repos living under `~/Documents`, `~/Desktop`, or network
> volumes, that sweep trips a macOS permission prompt **per protected location**,
> every time grants reset (the app is unsigned — #230), which reads as "agentum is
> aggressively scanning my machine and asking for every permission". The same
> sweep makes the view slow to appear, generation progress is invisible (a 3 s
> poll, nothing rendered until the whole wiki is done), and the Wiki sits in the
> nav rail as a global destination even though a wiki only ever belongs to one
> project.
>
> ## Goal
>
> Make the Wiki a **project-scoped** surface — reached through a first-class
> Projects entry in the sidebar, probing only the project the user actually
> opened, with status pushed over the event bus and pages rendered as they are
> generated.
>
> ## Users / personas
>
> - **Mateo (multi-project operator), on app update day.** Ten repos registered
>   across `~/Documents` and a NAS volume; he clicks Wiki and macOS fires a
>   prompt storm for folders he never asked agentum to touch. He wants zero
>   surprise reads — only the project he opened.
> - **Anyone generating a wiki for a big repo.** Today they stare at a spinner
>   for up to 20 minutes (`WIKI_SETTLE_TIMEOUT = 1200s`) with no sign of life;
>   they want to see pages appear as the agent writes them.
>
> ## Acceptance criteria
>
> **F1 — Wiki off the rail, Projects on it**
>
> 1. The sidebar nav rail renders **no "Wiki" item**; `SidebarNav.tsx` has no
>    `openWikiPage` reference (it was the only caller — the standalone Wiki hub
>    with its `RepoRail` is removed as dead code, or unreachable and deleted).
>    Per D1, the mechanical trailers go too: `'wiki'` leaves the
>    `activeView`/`previousViewBefore*` unions (`ui.ts:440–447`), the
>    `openWikiPage`/`closeWikiPage` store actions (`ui.ts:1079–1088`;
>    `closeWikiPage` has zero callers today), the `App.tsx:1752` render arm,
>    the `resolve-zoom-target.ts:14` entry, and `SidebarNav.test.tsx:17`
>    (which lists `'wiki'` among inactive nav items) is updated.
> 2. A **Projects** group renders in the sidebar listing every repo in
>    `s.repos`; activating an entry calls `openProjectHub(repo.id)` →
>    `activeView === 'project'` with the existing pinned Chat / Wiki / Tasks /
>    Sessions tabs.
> 3. The Project Hub **Wiki tab** renders the wiki for the pinned repo only
>    (`<WikiPage pinnedRepoId={repo.id} />` unchanged); opening the hub issues
>    `getWiki` probes **for the pinned repo only** (no other repo probed),
>    asserted by the probe effect's unit test. (The *exactly-one-network-call*
>    assertion lands in F2/AC-4 — until the sweep is deleted, the pinned-scoped
>    sweep + `refreshIndex` legitimately hit the same repo twice.)
>
> **F2 — Quiet probing (privacy)**
>
> 4. No surface issues `GET /api/wiki` (or any git-remote resolution) for a repo
>    the user has not explicitly opened — the every-repo `sweep` in
>    `WikiPage.tsx` is deleted; a unit test on the probe logic asserts
>    one-repo-only, and hub open now issues **exactly one** `/api/wiki` read
>    (moved here from AC-3: it only holds once the sweep is gone).
> 5. `routes/wiki.rs::resolve_target` caches the repo→wiki-key resolution
>    in-memory per repo id: repeated `GET /api/wiki` calls (e.g. status checks
>    during a run) spawn **no additional** `git remote get-url` subprocess after
>    the first resolution (cache invalidated on repo path/connection change).
> 6. The fs-picker protected-dir guard (`routes/fs.rs::is_protected_media_dir`)
>    is extended so that **no default/prefetch listing** ever descends into
>    `Desktop`, `Documents`, `Downloads`, or a network-volume root — only an
>    explicit user click on that directory lists it (Pictures/Music/Movies keep
>    today's hard bail). Extended unit test mirrors
>    `protected_media_dirs_are_flagged_but_projects_are_not` (`fs.rs:631`).
>
> **F3 — Pushed status, progressive render (performance)**
>
> 7. Wiki generation status transitions (running → ready/failed, and each new
>    page written) **emit an event on the existing global `/api/events` bus**:
>    one kind, `wiki.updated`, payload
>    `{ repo_id: string, status: "running"|"ready"|"failed", pages?: string[] }`
>    (D4 — page writes re-emit `wiki.updated` with updated `pages` while
>    `status` stays `running`; no separate `wiki.page_written`; `Event.kind` is
>    an open dotted String, `agentum-core/src/lib.rs:429–437`, so no core enum
>    change). `WikiPage` subscribes and updates without polling — the 3 s
>    `RUNNING_POLL_MS` loop is removed; any retained poll is a **≥30 s fallback
>    firing only while the events socket is down**, never a parallel path.
> 8. While a generation is `running`, pages already written to the wiki dir
>    render in the TOC behind a visible "generating…" banner (progressive
>    render); the state flips to `ready` only when `index.json` validates and
>    all pages are present — the loud-failure contract (001 AC-9:
>    missing/garbled index → error, never a half-empty "success") is preserved
>    and its tests still pass.
> 9. Existing gates stay green: `cargo test -p agentum-server --lib`,
>    `npm run build --prefix crates/agentum-desktop/ui`, spec-scoped vitest
>    suites.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** sidebar restructure (Wiki out, Projects group in); hub-only Wiki
>   access; one-repo probing + key cache; protected-dir guard widening for
>   automatic reads; event-bus wiki status; progressive page render.
> - **Out:**
>   - **App signing / notarization** (#230) — the reason TCC grants reset on
>     every update. This spec removes the *triggers*; the recurrence fix is
>     signing, tracked separately.
>   - Making the LLM generation run itself faster (model/prompt tuning) — the
>     agent run dominates cost and is out of scope; we fix *perceived* latency.
>   - Phase-2 candle neural embeddings (spec 003) and any RAG changes.
>   - SSH/remote wiki **generation** (still local-only, `routes/wiki.rs:252`).
>   - Cross-repo wiki, in-app wiki editing, full-text search.
>   - Reworking `WorktreeList`'s existing repo grouping/headers beyond adding
>     the Projects group (no sidebar redesign).
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **Project Hub with a pinned Wiki tab** — `ProjectHubPage.tsx:41–46` (tabs)
>   and `:181` (`<WikiPage key={repo.id} pinnedRepoId={repo.id} />`); hub open
>   action `openProjectHub(repoId, tab?)` (`store/slices/ui.ts:456, 876–889`)
>   already accepts a target tab. F1 is mostly *removal* + a nav group.
> - **Pinned-repo probe path** — `WikiPage.tsx:97–103` (`pinnedRepoId` prop) and
>   `:177` (single-repo probe): the quiet path already exists; the noisy one
>   (`sweep`, `WikiPage.tsx:175–189`, N × `getWiki`) is what gets deleted.
> - **The one Wiki rail entry** — `SidebarNav.tsx:219` (+ selector `:104`,
>   `wikiActive` `:195`); `openWikiPage`/`closeWikiPage` (`ui.ts:1079–1088`)
>   and `activeView === 'wiki'` render arm (`App.tsx:1752`) become dead once it
>   goes.
> - **Sidebar project rows that already open the hub** — `WorktreeList.tsx:3049–3052`
>   (click) and `:3076–3078` (Enter): precedent for the Projects group's
>   behavior and a11y.
> - **Global event bus** — `routes/events.rs` (`/api/events` WS broadcast) and
>   the UI's existing events subscription (the sidebar watchdog events ride it
>   today): `wiki.updated` is one more event kind, not a new transport.
> - **Wiki status discriminator + loud failure** — `.status.json` writing in
>   `routes/wiki.rs:316, 363–379`, `load_index_response`/`all_pages_present`
>   (`routes/wiki.rs:489–540`), `parse_wiki_index` (`wiki.rs:115`): progressive
>   render layers on top; the ready/failed semantics don't change.
> - **Protected-dir guard + test pattern** — `routes/fs.rs:116–136`
>   (`is_protected_media_dir`, rationale comment `:107–114`), enforcement at
>   `:158–167` and `:278–287`, test at `:631–659`: F2's guard widening extends
>   this, same shape.
> - **One launch path** — wiki generation already spawns via
>   `spawn_agent_into_pane` (`routes/wiki.rs:314`) with the YOLO marker
>   (`:302`): untouched by this spec.
>
> ### Build new
>
> - **Sidebar Projects group** — a compact nav group (one row per repo →
>   `openProjectHub`) in `SidebarNav.tsx` / a small new component; reuses the
>   repo store + WorktreeList row behavior.
> - **`resolve_target` key cache** — an in-memory `repo_id → wiki key` map in
>   `routes/wiki.rs` (invalidate on repo mutation), removing the per-call
>   `git remote get-url` subprocess.
> - **`wiki.updated` event emission** — emit on the state broadcast bus from the
>   generate background task's transitions (`routes/wiki.rs:328–379`) and from
>   page-write detection; UI subscription in `WikiPage`/wiki-client replacing
>   `RUNNING_POLL_MS` (`WikiPage.tsx:54, 250–254`).
> - **Progressive running-state render** — during `running`, list `<slug>.md`
>   files present in the wiki dir (server: extend the status payload; UI: TOC +
>   banner) without flipping the discriminator.
> - **Guard widening** — `is_protected_media_dir` companion (e.g.
>   `is_click_to_open_dir`) for Desktop/Documents/Downloads/`/Volumes/*` +
>   enforcement only on default/prefetch paths (explicit navigation still
>   lists).
>
> ## Risks & invariants
>
> - **Push-based, never poll** (architecture principle 3): F3 *aligns* with
>   this — replacing the 3 s HTTP poll with the event bus. Don't ship the event
>   without removing/demoting the poll.
> - **Loud failure is sacred** (001 AC-9): progressive render must not create a
>   path where a garbled/missing index reads as success; `ready` still requires
>   a validated index + all pages.
> - **Don't over-block Documents/Desktop:** repos legitimately live there. The
>   guard widening applies to *automatic* reads only — an explicit click must
>   still list, or workdir picking breaks (the Pictures/Music/Movies hard-bail
>   stays because repos don't live there).
> - **Residual prompts are not zero:** the generation agent itself (YOLO Claude
>   in the repo workdir) will still trigger one TCC prompt if the repo lives in
>   a protected folder — that read is the user's explicit intent, and recurrence
>   across updates is #230 (signing), not this spec.
> - **Embed contract:** `WikiPage`'s `pinnedRepoId` path is the survivor; make
>   sure removing the standalone hub doesn't strip props/state the ProjectHub
>   embed depends on (`ProjectHubPage.tsx:181`).
> - **Cache staleness:** the `resolve_target` cache must invalidate when a
>   repo's path or connection changes, or a moved repo would read another
>   repo's wiki (key is the normalized git remote — `wiki.rs:58–63`).
> - **One launch path / YOLO translation:** untouched; any change to the
>   generate spawn is out of scope.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries** (ordered, one gated slice each):
>   1. `projects-sidebar-wiki-off-rail` — F1 (AC 1–3).
>   2. `wiki-quiet-probing` — F2 (AC 4–6).
>   3. `wiki-push-status-progressive` — F3 (AC 7–8).
> - **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green
>   (guard-widening test, key-cache test, event-emission test, loud-failure
>   regression); `npm run build --prefix crates/agentum-desktop/ui` green;
>   spec-scoped vitest (probe-once, poll-removed, progressive-TOC reducer)
>   green.
> - **`qa.sh` asserts (browser QA)** — scoped per `$HARNESS_FEATURE_ID`:
>   - `projects-sidebar-wiki-off-rail`: sidebar shows no Wiki rail item; a
>     Projects group lists repos; clicking one opens the Project Hub; the Wiki
>     tab renders the pinned repo's wiki.
>   - `wiki-quiet-probing`: with devtools network open, opening the hub Wiki
>     tab issues `/api/wiki?repoId=<pinned>` reads **only for the pinned repo**
>     (at most two on mount: the probe + the events-socket onOpen refetch) and
>     **zero** reads for any other repo. Production build (dev StrictMode
>     double-invokes effects). [Amended per tester ruling 2026-07-06 —
>     `verification.md` §C; original wording said "exactly one read".]
>   - `wiki-push-status-progressive`: Generate → pages appear progressively
>     under a "generating…" banner → final `ready` renders the full TOC.
>   Screenshot evidence per `browser-verification-loop`.
>
> ## PM decisions (locked 2026-07-06 — constraints, not options)
>
> - **D1 — Standalone wiki view is deleted entirely**: the `activeView==='wiki'`
>   arm, `openWikiPage`/`closeWikiPage` store actions, and union entries go.
>   Verified: `SidebarNav.tsx:219` is the only `openWikiPage` caller and
>   `closeWikiPage` has zero callers — a hidden deep-link redirect would be
>   dead code by construction. `WikiPage` survives as the ProjectHub embed.
> - **D2 — Projects group = a separate compact, always-visible rail section**,
>   each row → `openProjectHub(repo.id)`. `groupBy` is user-toggleable, so
>   hanging access off `groupBy === 'repo'` (`SidebarHeader.tsx:19`) would make
>   it vanish under other groupings. Reuse the WorktreeList row click/Enter
>   a11y precedent (`WorktreeList.tsx:3050/:3077`).
> - **D3 — AC-6 contingency confirmed**: PM's own audit found no other
>   default/prefetch read of protected dirs (`fs.rs` greps clean; the picker
>   lists names without descending) — AC-6 is expected to land as regression
>   guard + tests; the PR must state the build-time audit result explicitly.
> - **D4 — One event kind `wiki.updated`**, payload
>   `{ repo_id, status: "running"|"ready"|"failed", pages?: string[] }`;
>   page writes re-emit with updated `pages` while `status` stays `running`.
>   Matches the bus's open dotted-kind convention (`session.crashed`,
>   `watchdog.compact`) and snake_case payload keys; one kind = one
>   subscription + a trivially testable reducer.
>
> ### Architect notes (from PM, not send-backs)
>
> - **AC-5 cache**: prefer a self-invalidating key `(repo_id, path,
>   connection_id)` over an invalidation callback — there may be no repo-mutation
>   hook to hang invalidation on, and a stale key reads *another repo's wiki*.
> - **Page-write detection** (fs-notify vs server-local scan while `running`) is
>   the architect's pick; a server-local scan does not violate push-not-poll
>   (that invariant governs client↔server) as long as the client is event-driven.
